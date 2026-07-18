# Preset Authoring Guide

本指南帮助你创建、验证和维护自己的 Ralph 工作流配置（preset）。

## 概念澄清

Ralph 有三个不同的配置层：

| 层 | 来源 | 用途 | 使用方式 |
|---|---|---|---|
| **Builtin Preset** | Ralph 内置，通过 `-H builtin:<name>` 加载 | 产品运行面，Ralph 官方维护 | `ralph run -H builtin:debug -p "..."` |
| **Template** | `ralph preset` 命令使用的脚手架 | 用来生成本地 preset 的起点 | `ralph preset new <template> ...` |
| **Local Preset** | 你自己创建的 YAML 文件 | 你自己的 workflow，自己维护 | `ralph run -H .ralph/hats/my.yml -p "..."` |

**关键区别：**
- Template 是**脚手架**，不是产品运行面。模板不会自动变成 builtin。
- Template 生成的 Local Preset 是**普通 YAML**，可以被 `ralph run` 加载，但 `x_preset` 元数据只在 `ralph preset` 命令中读取。
- `x_preset` 元数据**不影响运行时行为**。RalphConfig 解析器会忽略它。

## 推荐流程

### 1. 发现模板

```bash
# 查看所有可用模板
ralph preset list

# 查看模板详情
ralph preset show minimal-linear
```

可用模板：

| 模板 | 描述 | 难度 | 适合场景 |
|---|---|---|---|
| `minimal-linear` | 极简二 hat 线性流程 | 入门 | 学习/小工作流 |
| `debug` | 科学方法调试 | 中级 | Bug 调查和根因分析 |
| `ce-executor-lite` | 简化版 plan-driven 执行 | 中级 | 串行计划执行 |

### 2. 生成本地 Preset

```bash
# 基本用法
ralph preset new minimal-linear --name my-flow --output .ralph/hats/my-flow.yml

# 带描述
ralph preset new minimal-linear --name my-code-flow --description "Team debug workflow" --output .ralph/hats/my-code-flow.yml

# 生成后自动检查（推荐）
ralph preset new minimal-linear --name my-code-flow --output .ralph/hats/my-code-flow.yml --check
```

**生成的 YAML 包含 `x_preset` 元数据块：**

```yaml
x_preset:
  schema_version: 1
  template: minimal-linear
  template_version: "1.0.0"
  generated_by: "ralph preset new"
  generated_at: "2026-06-08T00:00:00Z"
  name: my-code-flow
  description: "Team code assist workflow"
  check_profile: strict
  ralph_compat: ">=0.2.0"

hats:
  # ... 实际 hat 配置
```

### 3. 验证 Preset

生成后（或修改后）验证：

```bash
# 基本检查
ralph preset check -H .ralph/hats/my-flow.yml

# 严格模式（推荐用于 CI 和 PR gate）
ralph preset check -H .ralph/hats/my-flow.yml --strict

# JSON 输出（用于自动化）
ralph preset check -H .ralph/hats/my-flow.yml --format json
```

`ralph preset check` 检查四个方面：
- **config** — RalphConfig 语义警告和错误
- **topology** — 起始事件、完成承诺、必需事件的可达性
- **orphan** — 发布的 topic 没有特定订阅者（拼写错误和陈旧发布）
- **payload** — 声明的 schema vs 下游 hat 实际引用的字段

### 4. 运行 Workflow

```bash
# 使用你生成的 preset
ralph run -c ralph.yml -H .ralph/hats/my-flow.yml -p "实现用户认证"
```

## 版本化和升级

### 查看本地 Preset 与模板的差异

```bash
ralph preset diff --file .ralph/hats/my-flow.yml
```

### 预览升级信息

```bash
ralph preset upgrade --file .ralph/hats/my-flow.yml --dry-run
```

**注意：** MVP 版本（`ralph preset upgrade`）只支持 dry-run，不自动写回。复杂用户改动需要人工合并。

### 没有 `x_preset` 的旧 Preset

手写的旧 preset 没有 `x_preset` 元数据：
- 仍然可以正常运行
- `ralph preset diff` 和 `ralph preset upgrade` 会给出友好提示
- 建议用 `ralph preset new <template> --name <name>` 重新生成，然后用 `ralph preset diff` 对比差异

## x_preset 元数据说明

`x_preset` 是机器可读的元数据，记录 preset 的来源和版本：

| 字段 | 类型 | 描述 |
|---|---|---|
| `schema_version` | 整数 | 元数据格式版本，当前为 `1` |
| `template` | 字符串 | 模板名称，如 `minimal-linear` |
| `template_version` | 字符串 | 模板版本，SemVer 格式 |
| `generated_by` | 字符串 | 生成工具，如 `ralph preset new` |
| `generated_at` | 字符串 | RFC3339 时间戳 |
| `name` | 字符串 | 用户指定的 preset 名称 |
| `description` | 字符串 | 用户描述 |
| `check_profile` | 字符串 | 推荐检查级别：`authoring` / `strict` |
| `ralph_compat` | 字符串 | 兼容的 Ralph 版本范围（可选） |

**重要：** `x_preset` **不是运行时指令**。RalphConfig 解析器会忽略未知顶层字段，所以 `x_preset` 不影响 `ralph run` 行为。

## 安全说明

### 占位符白名单

模板渲染只支持白名单中的占位符：

| 占位符 | 描述 |
|---|---|
| `{{preset_name}}` | 用户指定的 preset 名称 |
| `{{description}}` | 用户描述 |
| `{{author}}` | 作者信息（可选） |
| `{{generated_at}}` | RFC3339 时间戳 |
| `{{starting_event}}` | 起始事件（可选） |
| `{{completion_promise}}` | 完成承诺（可选） |

**未知占位符会报错**，不会静默替换。这避免了任意模板代码执行。

### 模板渲染不执行代码

- 不读取远程模板
- 不运行 shell 命令
- 不执行任意表达式
- 不引入通用模板引擎

## 常见问题

### Q: 模板会自动变成 builtin preset 吗？

不会。模板是脚手架，生成的 local preset 是普通 YAML 文件。两者的维护链完全分开。

### Q: 我可以修改生成的 preset 吗？

可以。生成的 preset 是普通 YAML，你可以随意修改。`x_preset` 元数据只是记录来源，不限制修改。

### Q: 升级会自动合并我的改动吗？

MVP 版本不会。`ralph preset upgrade --dry-run` 只输出建议，需要人工合并。自动合并复杂 YAML 改动风险太高。

### Q: `ralph preset check` 和 `ralph hats validate` 有什么区别？

| 命令 | 用途 |
|---|---|
| `ralph hats validate` | Hat 调试专用，检查拓扑和连通性 |
| `ralph preset check` | Preset 作者入口，检查 config/topology/payload/orphan 四个维度 |
| `ralph preflight` | 环境 + 配置检查，在运行前执行 |

### Q: 旧的手写 preset 能用 `ralph preset diff` 吗？

不能。`diff` 需要 `x_preset` 元数据来找到对应的模板基线。旧 preset 没有元数据，会收到友好提示。

## Migrating to topic_format gate（迁移指南）

> 适用于 **2026-06 之后**首次启动 `topic_format` 静态门禁的 preset。作者升级旧配置或把项目里的 `ralph.yml` 接到新版时，请按本节顺序检查。

### 背景

`preset-static-lint` 计划（参见 [plan 003](../plans/2026-06-08-003-feat-preset-static-lint-plan.md)）引入了一个启动硬门禁：在 `ralph run`、`hats validate`、`preset check --strict` 三个入口共享同一份 contract report，对以下三类问题执行 **fail-closed**：

1. **topic 命名格式**：所有 topic 必须满足 lowercase dot-case（例如 `work.done`）。遗留协议 token（如 `LOOP_COMPLETE`、`MERGE_COMPLETE`）用显式 `topic_format_whitelist` 列出豁免，不再"靠约定兼容"。
2. **owner 独占语义**：`topic_owners[*]` 中声明的 hat 必须出现在该 hat 的 `publishes` / `default_publishes` 里；非 owner hat 不允许发布 owner topic。
3. **coordinator 闭环**：当 `tasks.enabled=true` 时，所有发布 `task.*` 的 hat 都必须出现在 `tasks.coordinator_hats` 列表里。

升级到这个版本后，**默认配置（既有 `LOOP_COMPLETE` 协议字 + 默认 `tasks.enabled=true`）会在 backend 启动之前直接 fail-fast**，不会进入正常的 event loop。这是有意的——避免在错误的拓扑上烧 token。本节给出三步迁移模板，让你的配置"能跑过 strict gate"。

### 常见 3 步迁移

#### Step 1：加 `topic_format_whitelist`

把你的 preset 中所有**协议级 topic token**（非业务事件，但被 orchestrator 识别）显式列入白名单。最常见的候选是 `LOOP_COMPLETE`，在 `merge-loop` preset 里还需要加 `MERGE_COMPLETE`。

**Before：**

```yaml
event_loop:
  starting_event: "ralph.start"
  completion_promise: "LOOP_COMPLETE"
```

**After：**

```yaml
event_loop:
  starting_event: "ralph.start"
  completion_promise: "LOOP_COMPLETE"
  topic_format_whitelist:
    - LOOP_COMPLETE       # completion promise 用到
    # - MERGE_COMPLETE    # 仅 merge-loop 系列 preset 需要
```

> **判断标准**："这个 token 是不是 orchestrator 内部的协议字（ALL_CAPS），而不是我业务上要 listen / publish 的事件？" 如果是 → 加白名单；不是 → 把它改成 `lowercase.dot.case`（例如 `WorkComplete` → `work.complete`）。

#### Step 2：补 `coordinator_hats`（当 `tasks.enabled=true`）

只有当你显式启用了任务协议（`tasks.enabled=true`）时才需要这一步。如果你不使用 task system，**把 `tasks.enabled` 关掉**也可以绕过：

```yaml
tasks:
  enabled: false
```

如果你使用 task system 并且想让 lint 通过，需要把 **所有发布 `task.*` topic 的 hat** 列进 `coordinator_hats`。lint 会自动检测 candidate hats 并在缺 coordinator 时把它们列在错误信息里。

**Before：**

```yaml
tasks:
  enabled: true
hats:
  planner:
    triggers: ["ralph.start"]
    publishes: ["task.create", "plan.done"]
  executor:
    triggers: ["plan.done"]
    publishes: ["task.update", "work.done"]
```

**After：**

```yaml
tasks:
  enabled: true
  coordinator_hats:
    - planner             # 发布 task.create
    - executor            # 发布 task.update
hats:
  planner:
    triggers: ["ralph.start"]
    publishes: ["task.create", "plan.done"]
  executor:
    triggers: ["plan.done"]
    publishes: ["task.update", "work.done"]
```

> 没有 `coordinator_hats` 但又启用了 `tasks.enabled` → lint 会抛 `preset.coordinator_missing` 并附带 candidate list（lint 启发式找出来的"看起来是 task 协调者"的 hats）。把它照搬到 `coordinator_hats` 通常就能过。

#### Step 3：给每个 hat 补 `terminal_events`（可选但推荐）

`terminal_events` 告诉 orchestrator "这个 hat 完事之后才算真正结束"。缺省时 lint 报 `config.empty_terminal_events`（Warn 级别，strict 模式下升级为 Error），同时 topology 也无法证明"completion promise 一定可达"。

**Before：**

```yaml
hats:
  executor:
    triggers: ["plan.done"]
    publishes: ["work.done"]
```

**After：**

```yaml
hats:
  executor:
    triggers: ["plan.done"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]   # executor 结束 = work.done 落地
```

> **判断标准**：每个 hat 的 `terminal_events` 就是它"对世界宣告自己干完了"的那条 publish。一个 hat 通常有一条 terminal event；如果你有并发 + aggregate 的扇出场景，每个 wave worker 自己有一个 terminal event 即可。

### 常见 finding id 速查

> ID 是稳定的机器字符串（snake_case，前缀按 source 分类）。在 `ralph preset check --format json` 输出、`RuntimeContractFinding.id`、CI 报告里都能直接 grep。

#### Topic format（plan 003 / U3）

| finding id | 含义 | 怎么改 |
|---|---|---|
| `preset.invalid_topic_format` | topic 不符合 `lowercase.dot.case`，且不在 whitelist | 重命名为 `lowercase.dot.case`；或加入 `topic_format_whitelist` |
| `preset.whitelist_exempt_topic` | whitelist 中的 token 被识别为 protocol token | **无需改**——这是 Pass severity 的提示，确认豁免正确即可 |

#### Ownership（plan 003 / U2 + R2–R4）

| finding id | 含义 | 怎么改 |
|---|---|---|
| `preset.owner_unknown_hat` | `topic_owners[topic]` 引用了不存在的 hat | 在 `hats:` 加这个 hat，或从 `topic_owners` 移除它 |
| `preset.owner_not_publisher` | declared owner hat 没有把该 topic 写进 `publishes` / `default_publishes` | 把 topic 加进 owner hat 的 `publishes`（或 `default_publishes`） |
| `preset.cross_hat_unauthorized_publish` | 非 owner hat 在 publish 一个被 owner 独占的 topic | 从该 hat 的 `publishes` 里删掉这个 topic；或把它加入 `topic_owners` |
| `preset.missing_topic_owner` | `topic_owners[topic]` 存在但**没有任何 hat publish 它** | 把该 topic 加进至少一个 owner hat 的 `publishes` |

#### Coordinator（plan 003 / U2 + R5）

| finding id | 含义 | 怎么改 |
|---|---|---|
| `preset.coordinator_missing` | `tasks.enabled=true` 但 `coordinator_hats` 为空 | 填写 `tasks.coordinator_hats`；或关掉 `tasks.enabled` |
| `preset.task_publisher_not_coordinated` | 有 hat publish `task.*` 但不在 `coordinator_hats` 里 | 把该 hat 加进 `coordinator_hats` |

#### Config（runtime contract aggregator）

| finding id | 含义 | 怎么改 |
|---|---|---|
| `config.terminal_topic_not_in_publishes` | `terminal_events` 里的 topic 没在 hat 的 `publishes` / `default_publishes` 出现 | 把它加进 `publishes`，或换一个确实会发布的 topic |
| `config.empty_terminal_events` | hat 没有 `terminal_events`（default / strict 都报） | 给 hat 加 `terminal_events`；或确认这个 hat 不会"自然结束"（罕见） |
| `config.invalid_completion_promise` | `completion_promise` 不在白名单格式 | 把 token 改成 `lowercase.dot.case`，或加进 `topic_format_whitelist` |
| `config.reserved_trigger` | hat 的 `triggers` 用了 orchestrator 保留的协议 topic | 换一个业务 topic；或确认确实需要（保留字列表见 `config/reserved_topics.rs`） |
| `config.invalid_concurrency` | `concurrency` 字段值非法 | 取值范围 1..=max_workers（默认上限见 config loader） |

#### Topology

| finding id | 含义 | 怎么改 |
|---|---|---|
| `topology.unreachable_start` | `starting_event` 不在任何 hat 的 `triggers` | 给至少一个 hat 的 `triggers` 加上 `starting_event` |
| `topology.unreachable_completion` | `completion_promise` 没有 hat publish 它 | 把 `completion_promise` token 加进某个 hat 的 `publishes`（或白名单） |
| `topology.unreachable_required` | `required_events` 中的 topic 在拓扑里不可达 | 让某个 hat 在路径上 publish 它 |
| `topology.required_event_not_on_all_paths` | 某条路径上不会 emit required event | 调整 hat 的 `publishes`，确保每条路径都覆盖 |

#### Orphan / Payload

| finding id | 含义 | 怎么改 |
|---|---|---|
| `orphan.no_subscriber` | 有 hat publish 的 topic 没有触发任何 hat（"孤儿"） | 给某个 hat 的 `triggers` 加上这个 topic；或从 publish 方移除 |
| `payload.schema_missing_for_required_topic` | required topic 没有声明 payload schema | 在 `event_loop.event_policy.schemas` 里补 schema |
| `payload.field_missing_from_schema` | hat 用到的 payload 字段不在 schema | 在 schema 里加这个字段（type / required） |

### 豁免（exempt_findings）机制

极少数 preset 因为**有意的设计取舍**无法 100% 满足 strict lint。最常见的例子是 `merge-loop`：

- `MERGE_COMPLETE` 是 loop 逻辑 emit 的，不是任何 hat publish 的（`topology.unreachable_completion`）
- `cleanup.done` 是内部实现细节，没有外部 subscriber（`orphan.no_subscriber`）
- `failure_handler` hat 没有 `terminal_events`（`config.empty_terminal_events`）

这些 preset 在 `crates/ralph-cli/src/presets.rs` 的 `exempt_findings` 列表里登记豁免，CI 才能放行。**注意：`exempt_findings` 是 builtin preset 维护者的逃生通道，不是给用户配置用的。**

用户 preset 不要靠 `exempt_findings` 绕过 lint——应当按上面 3 步迁移把配置改对。如果你必须使用 `exempt_findings`，请同时打开一个 issue 说明为什么不能用正常 3 步修复。

> 详见 plan 003 的 **R10**（`docs/plans/2026-06-08-003-feat-preset-static-lint-plan.md` 的 U5 "内置 preset strict 迁移"段）。R10 要求 manifest 中所有 9 个嵌入 preset 通过 strict lint；只有带"已知设计取舍"的 preset 才允许走豁免名单。

### 迁移完成自检

完成上述 3 步后跑一遍：

```bash
# 1. strict lint 通过（应该没有 finding / 只有 Pass）
ralph preset check -H .ralph/hats/my-flow.yml --strict

# 2. JSON 输出检查所有 finding.id 都是 pass 或 whitelist-exempt
ralph preset check -H .ralph/hats/my-flow.yml --format json | jq '.findings[] | select(.severity != "pass")'

# 3. 真正启动一次，确认 gate 不再 fail-fast
ralph run -c ralph.yml -H .ralph/hats/my-flow.yml --skip-preflight -p "smoke migration"
```

如果第 3 步仍然以退出码 2 失败，stderr 里会打印完整的 finding list（按 `source` 分组：config / topology / payload / orphan）。回到上面的"finding id 速查表"逐条对照即可。

## Agent Skills（operator）

编写与评审 preset 时，使用仓库内 operator skills（非 loop 注入）：

| Skill | 用途 |
|---|---|
| [`skills/ralph-preset-author`](../skills/ralph-preset-author/SKILL.md) | 拓扑 + 逐 hat AAF 起草 + **Payload Contract 表**；产出 `preset-author-notes.md` 后再交 review |
| [`skills/ralph-preset-review`](../skills/ralph-preset-review/SKILL.md) | 独立 activation dry-run + **Payload Audit 表** + 机械 lint → `preset-review-report.md`（含按 runtime unblock 排序的 remediation） |

> 用户 `.ralph/hats/` 集合（create / inspect / validate user hat workflows）、以及 topology-debug / validate-routing 这类 user-only 责任不在任一 preset skill 范围内。

**两 skill 共同构成 agent-flow 闭环**：author 写 per-hat payload 合同（字段 / 值源 / 可见性 / 身份 / 下游消费），review 从 activated-hat 视角独立审计每个 emit topic 的字段可见性、值源可达性、运行时身份一致性、语义充分性与下游消费。两 skill 都不替代 `ralph preset check`——lint 只验 shape 与拓扑，看不见 / 算不出 / 决策字段语义不足这类问题由 audit 兜底。

AAF 模型 + Payload Audit 模型详见 [`skills/ralph-preset-common/references/agent-native-model.md`](../skills/ralph-preset-common/references/agent-native-model.md)；finding → severity 映射（含 `payload-content`）见 [`skills/ralph-preset-common/references/finding-rubric.md`](../skills/ralph-preset-common/references/finding-rubric.md)。

## 相关文档

- [Hat Collections](./presets.md) — builtin preset 和 hat collection 概览
- [Precheck Gates](./precheck-gates.md) — 可选的事件发射 LLM 关卡（`event_loop.precheck`）
- [CLI Reference](./cli-reference.md) — 完整命令行参考
- [Runtime Contracts](./runtime-contracts.md) — preset 检查的详细行为矩阵
- [Payload Contracts](./payload-contracts.md) — Schema metadata（`field_docs` / `examples` / `known_fields` / `trigger_context`）与 `--policy-check` 拒收后 5 个 agent-facing 字段如何读，以及 `## TRIGGER CONTEXT` 区块如何解读
- [单链 preset 开发手册](../handbook/serial-preset-development.md) — `ce-executor-pipeline*` 内嵌协议 SSOT 维护指南
- [Plan: preset-static-lint](../plans/2026-06-08-003-feat-preset-static-lint-plan.md) — R1–R12 需求与 U1–U6 实现拆分
