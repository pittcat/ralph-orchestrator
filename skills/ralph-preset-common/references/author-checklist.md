# Author Checklist

## 双阶段大脑（强制）

### 阶段 1：拓扑（作者视角 OK）

- [ ] 判定路径：local（`.ralph/hats/*.yml`）vs builtin（`presets/en/` + `presets/schemas/`）
- [ ] 记录 `execution_mode`；4+ hat → 必须 `isolated`
- [ ] 读 schema SSOT：`presets/schemas/<name>.yml`（builtin）或 preset 内 `event_policy.schemas`
- [ ] 画事件流（topic 箭头，非 prompt 流）
- [ ] 每条 handoff 边：上游 Q4 emit 字段 ↔ 下游 Q2 Observe 命令/字段
- [ ] `state_projection.actions` 与 emit payload 字段对齐
- [ ] 每个 agent-authored emit topic 的 `event_policy.schemas.<topic>` 已检查：required handoff / identity / verdict / count / path / reason 字段有 `field_docs`，高风险 topic 有不会伪造业务事实的 `examples`
- [ ] 若 hat `publishes` 含 `review.dimensions.complete`，`state_projection.actions_chain` 须有对应投影 action（否则下游 Q2 看不到 review 汇总）
- [ ] emitter 若 instructions 要求 `--triggered <hat>`，该 `<hat>` 必须在 preset `hats[]` 里声明（否则 runtime 拒收 `triggered_not_in_topology`）
- [ ] loop preset 中 `fix.done.next_review_plan` 必须是非空 object 合同；schema、example、fixer instructions 都不能允许 `null`
- [ ] `dim:*` / `dimension-reviewer` 只读 reviewer 若禁用 Edit/Write，不得声明 `docs/plans/` 写路径；review 产物写到 `.ralph/review/**`
- [ ] 可参考 `references/patterns.md`（仅拓扑阶段）

### 阶段 2：起草（单 hat agent 视角）

- [ ] 写 hat X 的 `instructions:` 时 **只扮演 hat X 的 agent**
- [ ] 逐 hat 填 AAF 五问表（模板见下）
- [ ] 禁止拓扑句式抄进 instructions
- [ ] Emitter hat：引用 `ralph-tools-opac`、`ralph-tools-emit` §5；强制 `--policy-check`
- [ ] Emitter hat：若 instructions 提到 payload / required fields / field shape / `ralph emit` / `ralph wave emit`，必须引用 `ralph-tools-emit`「Policy-Check 反馈」；不要复制 `field_docs` 表
- [ ] Recovery / correction 路径：引用 `ralph-tools-recovery-directives`（通用 bounded retry）；preset 内用**触发状态表**写专用动作，不复述 data skill 全文
- [ ] `task_id` / `task_key` / `step`：引用 `ralph-tools-tasks` red box
- [ ] 不复述 `ralph-tools*.md` 参数表
- [ ] **对每个 emit topic，按 payload audit 五列填行**（见下）—— schema 通过不等于字段可达
- [ ] **对每个 payload 字段，反查 schema metadata**：`field_docs.meaning/source/fill_rule` 与 Payload Contract 的值源、可见性、下游消费一致

## AAF 五问表模板（每 hat 必填）

```markdown
## Hat: <id>

- **Q1 使命:** …
- **Q2 输入 (Observe 命令 + 期望字段):** …
- **Q3 执行 (OPAC 命令序列):** Observe → Precheck → Apply → Confirm
- **Q4 输出 (topic + payload 合同):** 见下方 Payload Contract 表
- **Q5 交接 (emit 字段 → 下游 Observe 路径):** …
```

**不可交付信号：** 任一为空；含「待定」「同上」「上游会处理」。

## Payload Contract 表模板（每 emit topic 必填）

每个 hat 至少填一张；多 trigger 须按 trigger 拆分。

```markdown
### Hat: <id> — Payload Contract

| topic | 字段 | 类型 | 值源（哪条命令 / 哪段 projection / 哪个 trigger payload 字段） | 可见性证据（hat prompt 栈哪一段可见） | 身份检查（是否需要 live task_id） | 下游消费（下游哪个 hat 的哪个决策用到） | schema metadata |
|---|---|---|---|---|---|---|---|
| `<topic>` | `task_id` | string | `ralph tools task list` → 当前 active task | `## ORCHESTRATOR CONTEXT` | 必须 live；禁手写 | reviewer 决定后续 fix / block | `field_docs.task_id.source` 指向 live task list；`fill_rule` 禁手写 |
| `<topic>` | `verdict` | enum | 本 hat work 输出 | `## HAT IDENTITY` trigger payload | 不涉及 | 同上 | `field_docs.verdict.meaning` 解释判定语义；`allowed_values` 列枚举 |
| `<topic>` | summary field（如 `must_fix_now_count`） | 整数 | 当前 trigger payload 字段 | `## TRIGGER CONTEXT`（preset/schema `trigger_context.summary_fields` 声明） | 不涉及 | 本 hat 决定路由分支 | schema `trigger_context.summary_fields` 列声明字段；missing 渲染为 `<missing>` |
| `<topic>` | matched hint guidance | 字符串 | `trigger_context.routing_hints` 命中条目 | `## TRIGGER CONTEXT`「Matched routing hints」段 | 不涉及 | 本 hat 行动指导 | `routing_hints[*].label` 唯一；同 `exclusive_group` 内不可同时命中 |
```

**Trigger Context 审核项（适用于 trigger-consuming hats）**

- [ ] **声明位置**：`trigger_context` 写在 `event_policy.schemas.<topic>` 下，不要放在 `hats[*].instructions` 或独立 sibling。
- [ ] **`summary_fields` 值源**：每条声明字段必须来自 trigger payload，且在 schema `required_fields` ∪ `known_fields` ∪ `field_docs.keys()` ∪ `allowed_values.keys()` 内。
- [ ] **`known_fields` 缺口**：若 hint 条件或 summary 字段引用非 required 字段，须把它加入 schema `known_fields`，不要靠放宽 lint。
- [ ] **`routing_hints` 形状**：每条 hint 用 `conditions: [{field, op, value}]` 形式，`op` 仅允许 `eq` / `ne` / `gt` / `gte` / `lt` / `lte` / `exists` / `missing`；数值比较 op 的 `value` 必须是 number；`exists` / `missing` 不带 `value`。
- [ ] **`label` 唯一性**：同 topic 内 hint `label` 不重复；同 `exclusive_group` 内 hint 条件须可静态判定为互斥。
- [ ] **拓扑消费方**：只为 `hats[*].triggers` / `subscribes_to` 包含该 source topic 的 hat 注入；无消费者触发 `trigger_context_no_consumer`。
- [ ] **guidance 是 agent 行动**：hint `guidance` 用「你应该如何处理」表达，不要写 runtime 控制命令、不要修改 routing / 权限。
- [ ] **instructions 不复制**：hat `instructions` 只引用 `## TRIGGER CONTEXT` 区块，不要把 hint 条件值复制进 instructions。

**拒交付信号：**

- 字段值源写「上游会处理」「待定」「约定俗成」
- `task_id` / `task_key` / `step` 字段未标 `live required`
- 多 trigger hat 合并成一行（必须按 trigger 拆分差异）
- 决策字段（`verdict` / `reason` / `summary` / `next_action`）无下游消费说明
- 某字段无任何一行说明它对 hat 可见
- required handoff / identity / decision 字段没有 `field_docs`，也没有说明为什么现有 injected skill 已覆盖
- `examples` 填了业务结论占位（例如固定 `0` / `pass`），而不是安全示例

## 交 review 前门禁

- [ ] 每 hat 一张 AAF 表 + Payload Contract 表，写入 `preset-author-notes.md`
- [ ] hat 表数 == YAML hat 数；每 emit topic 都填了 Payload Contract 行
- [ ] Payload Contract 的 `schema metadata` 列已同步到 `presets/schemas/<name>.yml` 或 preset inline `event_policy.schemas`
- [ ] Emitter instructions 引用 `ralph-tools-emit`「Policy-Check 反馈」，不复制字段说明
- [ ] 自问：「若我只收到这份 instructions + Ralph 注入，能否做完 Q1？Q4 每个字段我能取到值吗？」
- [ ] 对 builtin 改动：列出 7 点同步清单（见下），不自动执行
- [ ] 建议调用 `ralph-preset-review`（不替代 `ralph preset check`）

## Hard questions — single-chain-first (2026-07-07-006 Unit 6)

任何 preset 起草阶段在「默认走单链」之前必须先回答以下 5 问：

1. **本 preset 的 unit 拆分能否由 executor 内部 subagent 完成？** ✓ / ✗ + ≤50 字理由
2. **任何业务 topic 是否超过一个消费者？** ✓ / ✗ + ≤50 字理由
3. **fallback 是否可能路由到 success？** ✓ / ✗ + ≤50 字理由
4. **是否有 hat 把 tasks / progress / recovery 当业务事实？** ✓ / ✗ + ≤50 字理由
5. **是否有 rescue hat 能改变业务链路？** ✓ / ✗ + ≤50 字理由

任一问 ✗ → 必须改写或显式说明为何单链无法表达（默认应迁移到 executor 内部 subagent）。见 `references/finding-rubric.md` 的「Single-chain-first audit」段。

## Builtin 7 点同步清单（摘要）

改 `presets/en/<name>.yml` 或 `presets/schemas/<name>.yml` 事件拓扑后，逐层检查：

1. `crates/ralph-core/src/event_loop/mod.rs` — step-close / completion 语义
2. `crates/ralph-core/src/preset_lint/` — 相关 lint 规则
3. `crates/ralph-core/tests/scenarios/*.yml` + `scenarios.rs`
4. `crates/ralph-core/src/config/loop_config.rs`、`preflight.rs`、`config_resolution.rs`
5. `crates/ralph-cli/src/presets.rs` + SSOT 测试
6. `presets/manifest.yml`、`presets/index.json`
7. `CLAUDE.md` / `AGENTS.md`、`.cursor/rules/multi-hat-isolation.mdc`、`scripts/ralph-zsh-plugin.zsh`

详见 `docs/handbook/serial-preset-development.md`。

## 产出物

| 文件 | 位置 |
|---|---|
| Preset YAML | `presets/en/<name>.yml` 或 `.ralph/hats/<name>.yml` |
| `preset-author-notes.md` | 与 preset 同目录（默认）；含 AAF 五问表 + Payload Contract 表 |
