# Cross-Project Bootstrap — Preset-Bound Suite

`ralph-project-bootstrap` 为每个 preset 生成一套互不覆盖的运行文件。对于
`modem-case-docs.yml`，产物是：

- `ralph.modem-case-docs.yml`
- `PROMPT.modem-case-docs.md`

启动时必须同时显式指定 config 与 preset：

```bash
ralph -c ralph.modem-case-docs.yml -H modem-case-docs.yml run
```

## 为什么必须生成 prompt 文件

通过 `-H` 加载的文件或 builtin preset 属于 hats source。Ralph 的
operator/preset 合并边界不会把 preset 的 `event_loop.prompt` 合并为最终运行
配置，因此 bootstrap 会读取完整 preset，把该字段的原始文本快照到
`PROMPT.<stem>.md`，并在 `ralph.<stem>.yml` 中显式设置
`event_loop.prompt_file`。

如果 preset 没有非空 `event_loop.prompt`，操作者又没有提供 plan 或外部
prompt，bootstrap 必须以 `preset_prompt_missing` 停止，不能生成依赖默认
`PROMPT.md` 的不完整套件。

## 内嵌 provenance

生成配置的 `_bootstrap:` 映射保存：

| 字段 | 作用 |
| --- | --- |
| `preset` | 原始 file preset 路径或 builtin id |
| `prompt_file` | preset 专属 prompt 路径 |
| `generator_version` | bootstrap 生成器版本 |
| `input_signature` | resolved preset 与生成输入的摘要 |
| `profile_sha256` | 不含 provenance 行的生成配置摘要 |
| `prompt_sha256` | prompt 快照摘要 |

不再生成 `ralph.bootstrap.yml`。再次 bootstrap 时，必须先用
`reconcile_preset_bound_suite` 核对两个摘要：输入变化且现有文件仍与摘要一致时
可以安全刷新；人工修改任一受管文件时返回 `owned_value_user_modified`，停止
覆盖。

## 验证要求

静态校验顺序为 preset strict check、strict preflight、dry-run。每条命令都要
显式携带 `-c ralph.<stem>.yml -H <preset>`。dry-run 输出的 prompt 来源必须
精确等于 `PROMPT.<stem>.md`；回落到 `PROMPT.md` 即为失败。

green dry-run 只证明配置与 prompt 来源解析正确，不证明 loop 已闭环。真实或
mock backend smoke 仍需操作者授权；只有 skill 自带的固定 replay harness 可以
自动运行。
