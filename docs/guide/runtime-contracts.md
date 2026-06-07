# Runtime Contracts

Runtime Contracts 是 Ralph 的 preset/workflow 质量门禁体系。它把分散在 config、topology、payload、orphan 的校验能力收敛到统一的报告结构和 CLI 入口。

## 命令一览

| 命令 | 用途 | 检查内容 |
|------|------|---------|
| `ralph preset check` | Preset 作者入口 | config + topology + payload + orphan |
| `ralph hats validate` | Hat 调试入口 | topology + payload + orphan（保留旧行为） |
| `ralph preflight` | 运行前检查 | config + environment + topology + contract |
| `ralph run` payload hard gate | 运行时保护 | 静态 payload contract（不可跳过） |

## 推荐工作流

编辑或新建 preset 后，按以下顺序检查：

```bash
# 1. 检查 preset 结构是否健康
ralph preset check -H builtin:ce-executor --strict

# 2. 查看拓扑图（可选）
ralph hats graph -H builtin:ce-executor

# 3. 检查运行环境
ralph preflight

# 4. 运行 workflow
ralph run -H builtin:ce-executor -p "your prompt"
```

## Strict 模式

不同入口的 `--strict` 语义不同：

| 入口 | `--strict` 效果 |
|------|----------------|
| `ralph preset check --strict` | `payload_strict=true` + `fail_on_warnings=true` |
| `ralph hats validate --strict` | 仅 `payload_strict=true` |
| `ralph preflight --strict` | 仅 `fail_on_warnings=true` |

- **payload_strict**: payload missing schema 从 warning 升级为 error
- **fail_on_warnings**: warning 也导致整体失败

## JSON 输出

不同入口的 `--format json` 输出的 JSON 结构不同——这是有意为之：
两个入口要表达的内容不同，共享一份结构会迫使其中一方在表达不准确时妥协。

| 入口 | 输出类型 | 顶层字段 |
|------|---------|---------|
| `ralph preset check --format json` | `RuntimeContractReport` | `source_label`、`payload_strict`、`fail_on_warnings`、`passed`、`warnings`、`errors`、`findings[]`、`checked_at` |
| `ralph preflight --format json` | `PreflightReport` | `passed`、`warnings`、`failures`、`checks[]` |

`ralph hats validate` 目前只输出人类可读格式（plain text），不支持 `--format json`。

### `ralph preset check --format json`

```json
{
  "source_label": "builtin:ce-executor",
  "payload_strict": false,
  "fail_on_warnings": false,
  "passed": true,
  "warnings": 0,
  "errors": 0,
  "findings": [],
  "checked_at": "2026-06-06T00:00:00Z"
}
```

`findings[]` 里每个 finding 包含：
- `id`: 稳定机器 ID（如 `topology.unreachable_completion`）
- `source`: 来源（`config` / `topology` / `orphan` / `payload`）
- `severity`: 严重程度（`pass` / `warn` / `error`）
- `stage`: 生命周期阶段（`authoring` / `preflight` / `run_hard_gate`）
- `message`: 人类可读摘要
- `details`: 可选结构化上下文
- `action_hint`: 可选修复建议

### `ralph preflight --format json`

```json
{
  "passed": true,
  "warnings": 1,
  "failures": 0,
  "checks": [
    {
      "name": "config",
      "label": "Configuration valid",
      "status": "pass"
    }
  ]
}
```

`checks[]` 里每条 check 包含 `name`（check 标识）、`label`（人类摘要）、
`status`（`pass` / `warn` / `fail`）。这是 preflight 早于 Runtime Contract
设计时的结构，承载 `config + environment + topology` 的运行前检查。

### 自动化消费指引

- 解析 `ralph preset check` 的输出时，按 `RuntimeContractReport` 形态处理
  `findings` 数组；按 `severity` + `fail_on_warnings` 推导最终 pass/fail。
- 解析 `ralph preflight` 的输出时，按 `PreflightReport` 形态处理 `checks`
  数组；按 `status` 推导每条 check 的结果，再根据 `--strict` 与 `warnings`/
  `failures` 推导整体 pass/fail。
- 不要把两者的 JSON 混为一谈；它们的字段名重叠（`passed` / `warnings`）
  但语义和 schema 不同。

## 默认行为说明

- `ralph run` **不会**默认运行 preflight。需要设置 `features.preflight.enabled: true`。
- `ralph run` 的 payload hard gate **不可跳过**，无论 preflight 是否启用。
- `features.preflight.skip` 可以跳过特定检查（如 `["preset-topology", "preset-contract"]`）。

## 批量检查

使用开发脚本批量检查所有 public builtin preset：

```bash
./scripts/validate-builtin-presets.sh           # 非 strict
./scripts/validate-builtin-presets.sh --strict  # strict 模式
```
