# Preset Validation Commands

## Preset 路径写法

| 类型 | `-H` 示例 |
|---|---|
| Builtin | `builtin:debug` |
| Repo 内 YAML | `presets/en/debug.yml` |
| Local | `.ralph/hats/my-workflow.yml` |

可加 `-c ralph.yml` 指定 core config（local preset 常用）。

## 机械门禁（review 默认）

```bash
# Preset runtime contract（config + lint + topology + orphan + payload）
ralph preset check -H <path|builtin:name> --strict
ralph preset check -H <path|builtin:name> --strict --format json

# Workspace preset_lint 子集
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
```

`--strict`：Warn 级 finding 也视为失败。JSON 输出供 review 报告 Mechanical Lint 节摘录。

## Schema / emit 验证

```bash
# 某 topic 的 payload 字段 SSOT
ralph emit --schema <topic> -H <path|builtin:name>

# 写盘前策略预检（OPAC Precheck）
ralph emit --policy-check <topic> '<payload>' -H <path|builtin:name>
```

## Hat 检查（local / 路径 preset）

```bash
ralph hats validate -c ralph.yml -H <hats.yml>
ralph hats show -c ralph.yml -H <hats.yml> <hat_id>
ralph hats graph -c ralph.yml -H <hats.yml> --format mermaid
```

`ralph hats show` 可看单 hat 有效配置；**不是**完整 isolated prompt dump。

## Preset 脚手架（author 拓扑阶段）

```bash
ralph preset list
ralph preset show <template>
ralph preset new <template> --output .ralph/hats/my.yml
ralph preset diff --file <path>   # 与 template 基线对比
```

## 合入前升级（非默认）

```bash
./scripts/run-tests.sh
cargo nextest run -p ralph-core --test scenarios
scripts/check-cli-doc-drift.sh --strict
```

## Lint 失败时 review 行为

机械 lint 失败时 **仍继续** AAF 评审；Executive Summary 须标注 lint 通过/失败及 Error 计数。
