# Cross-Project Bootstrap — Preset-Bound Suite

`ralph-project-bootstrap` 为每个 preset 生成一套互不覆盖的运行文件。对于
`modem-case-docs.yml`，产物是：

- `ralph.modem-case-docs.yml`
- `PROMPT.modem-case-docs.md`

启动时必须同时显式指定 config 与 preset：

```bash
ralph -c ralph.modem-case-docs.yml -H modem-case-docs.yml run
```

## 统一入口（operator contract）

bootstrap 全流程只有一个入口，操作者不应手工调用各个 helper 模块来拼装
流程。入口负责按严格顺序执行各阶段，并输出结构化结果：

```bash
python skills/ralph-project-bootstrap/scripts/bootstrap_pipeline.py \
  --cwd <project> --preset <preset> \
  [--plan <plan.md>] [--prompt-file <prompt.md>] \
  [--binary <ralph>] [--refresh-existing] \
  [--replay-transcript <transcript-dir>] [--json]
```

代码内等价调用为 `bootstrap_pipeline.run_pipeline(...)`。入口按以下顺序
短路执行：audit → preset 解析 → 生成并回读校验产物 → 静态校验
（capability → preset check → preflight → dry-run）→ 经授权的 replay
smoke → 类型化 handoff。

阶段顺序不可跳过；`--replay-transcript` 是唯一能启用自动 smoke 的开关
（仅 `content_fixed_replay` 有界回放后端；其它后端一律拒绝并不 spawn）。

入口输出一份 `PipelineResult`（默认文本视图，`--json` 输出同构 JSON），
并按三个验证等级决定退出码与操作者权限：

| 等级 | 含义 | 退出码 | 操作者动作 |
| --- | --- | --- | --- |
| `blocked` | 某阶段返回类型化阻塞（root 歧义、镜像冲突、provenance 损坏、静态门禁失败等） | 2 | 没有可执行命令；按结果中 `code` 指出的问题修复后重跑入口 |
| `incomplete_static_only` | 产物已生成且静态门禁全绿，但 loop 未闭环（未跑授权 smoke） | 0 | 命令带 `[CANDIDATE - operator must run manually]` 前缀（缺 plan 时为 `[TEMPLATE - replace PLAN_PATH before running]`）；操作者自行确认 backend 后手动执行，或带 `--replay-transcript` 重跑入口争取升级 |
| `complete` | 静态门禁全绿且有界 replay smoke 到达终态标记 | 0 | 命令为正式 launch 命令，可直接执行 |

**dry-run 全绿 != loop 已闭环**：green dry-run 只证明 runtime 能静态装载
该套件，不能证明 loop 能跑到终态；`incomplete_static_only` 表达的正是这一
状态。worktree 启动（`run_pipeline(..., use_worktree=True, ...)`）必须携带
显式复用键（`--plan <plan>` 或 `--worktree-name <name>`），缺失时入口以
`blocked` 视图拒绝，不会输出启动命令。

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
