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

所有支持 `--format json` 的命令输出稳定的 JSON 结构：

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

每个 finding 包含：
- `id`: 稳定机器 ID（如 `topology.unreachable_completion`）
- `source`: 来源（`config` / `topology` / `orphan` / `payload`）
- `severity`: 严重程度（`pass` / `warn` / `error`）
- `stage`: 生命周期阶段（`authoring` / `preflight` / `run_hard_gate`）
- `message`: 人类可读摘要
- `details`: 可选结构化上下文

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
