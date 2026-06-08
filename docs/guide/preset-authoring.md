# Preset Authoring Guide

本指南帮助你创建、验证和维护自己的 Ralph 工作流配置（preset）。

## 概念澄清

Ralph 有三个不同的配置层：

| 层 | 来源 | 用途 | 使用方式 |
|---|---|---|---|
| **Builtin Preset** | Ralph 内置，通过 `-H builtin:<name>` 加载 | 产品运行面，Ralph 官方维护 | `ralph run -H builtin:code-assist -p "..."` |
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
ralph preset show code-assist --format yaml
```

可用模板：

| 模板 | 描述 | 难度 | 适合场景 |
|---|---|---|---|
| `minimal-linear` | 极简二 hat 线性流程 | 入门 | 学习/小工作流 |
| `code-assist` | TDD 实现工作流 | 中级 | 默认实现任务 |
| `debug` | 科学方法调试 | 中级 | Bug 调查和根因分析 |
| `research` | 只读代码探索 | 入门 | 不修改代码的分析 |
| `review` | 代码审查 | 入门 | 不修改代码的审查 |
| `ce-executor-lite` | 简化版 plan-driven 执行 | 中级 | 串行计划执行 |

### 2. 生成本地 Preset

```bash
# 基本用法
ralph preset new minimal-linear --name my-flow --output .ralph/hats/my-flow.yml

# 带描述
ralph preset new code-assist --name my-code-flow --description "Team code assist workflow" --output .ralph/hats/my-code-flow.yml

# 生成后自动检查（推荐）
ralph preset new code-assist --name my-code-flow --output .ralph/hats/my-code-flow.yml --check
```

**生成的 YAML 包含 `x_preset` 元数据块：**

```yaml
x_preset:
  schema_version: 1
  template: code-assist
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
| `template` | 字符串 | 模板名称，如 `code-assist` |
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

## 相关文档

- [Hat Collections](./presets.md) — builtin preset 和 hat collection 概览
- [CLI Reference](./cli-reference.md) — 完整命令行参考
- [Runtime Contracts](./runtime-contracts.md) — preset 检查的详细行为矩阵
