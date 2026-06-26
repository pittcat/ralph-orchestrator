# Profiles

> Profile 是把同一 preset 在不同场景下切成不同「风格」的轻量机制。本概念文档面向最终用户,讲解目录结构、激活顺序、与 `profiles.default` / `--profile` / `--no-default-profiles` 三者之间的协作。

## 一句话总结

把要追加到 hat `instructions` 的内容按 `<scope>:<name>/<preset>/<hat>.md` 形式组织,运行时按激活顺序拼接;`repo:` 走项目内 `ralph-profiles/`,`user:` 走 `~/.config/ralph/profiles/`。

## 为什么需要 Profile

Builtin preset 是编译进二进制的 YAML,同一 preset 在不同项目或场景下的「风格差异」(严格验证 / 快速原型 / 中文输出 / 团队强制 review 等)此前只能通过以下方式表达:

- Fork preset 文件 —— 失去与上游 builtin 同步的能力。
- 在 `ralph.yml` 的 `hats.<id>.extra_instructions` 中塞长 markdown —— 与 preset 内置内容混在一起,不易共享给团队。

Profile 把这些差异拆成小型 markdown 片段,放在 `ralph-profiles/`(repo 级,被 git 追踪)或 `~/.config/ralph/profiles/`(user 级,个人偏好),运行时叠加到 hat instructions 上,既不 fork preset,也明确区分 repo 偏好与个人偏好。

## 目录结构

```
<scope-base>/<profile-name>/<preset-name>/<hat-id>.md
```

- `<scope-base>`
  - `repo` → `<project-root>/ralph-profiles/`
  - `user` → `$XDG_CONFIG_HOME/ralph/profiles/`(优先),否则 `~/.config/ralph/profiles/`
- `<profile-name>`: 自由命名,只能包含字母 / 数字 / `_` / `-`(拒绝空、空白、路径分隔符与 `..`)
- `<preset-name>`: builtin preset 取 `HatsSource::Builtin(name)` 中的 `name`(如 `debug`);文件 hats 取文件 stem(如 `my-hats.yml` → `my-hats`)
- `<hat-id>.md`: 与当前 preset 中某个 hat 的 id 完全匹配

### Repo profile 示例

```
ralph-profiles/
├── strict/                 # profile name = "strict"
│   ├── debug/              # 适用 builtin:debug preset
│   │   ├── investigator.md
│   │   └── fixer.md
│   └── ce-executor-serial/ # 适用 builtin:ce-executor-serial preset
│       ├── executor.md
│       └── reviewer.md
└── chinese-style/          # profile name = "chinese-style"
    └── debug/
        └── investigator.md
```

调用:

```bash
ralph run -H builtin:debug --profile repo:strict
# → debug preset 的 investigator 与 fixer hat 的 instructions
#   末尾各追加 <ralph-profiles/strict/debug/{investigator,fixer}.md>
```

### User profile 示例

```
~/.config/ralph/profiles/
└── my-style/
    └── debug/
        └── investigator.md
```

调用:

```bash
ralph run -H builtin:debug --profile user:my-style
```

## 激活顺序

profile 片段的拼接顺序由激活顺序决定——**`profiles.default` 在前,CLI `--profile` 在后**,每个 profile 内部按 `.md` 文件名升序加载。

```yaml
# ralph.yml
profiles:
  default: repo:strict, user:my-style
```

```bash
ralph run -H builtin:debug \
  --profile repo:extra-checks \
  --profile user:local-prefs
```

实际激活顺序(每段在匹配 hat 的 instructions 末尾按顺序拼接):

1. `<ralph-profiles/strict/debug/*.md>`(文件名升序)
2. `<~/.config/ralph/profiles/my-style/debug/*.md>`(文件名升序)
3. `<ralph-profiles/extra-checks/debug/*.md>`
4. `<~/.config/ralph/profiles/local-prefs/debug/*.md>`

## 常用命令

| 命令 | 用途 |
|------|------|
| `ralph run --profile repo:strict` | 启动 loop 并叠加 repo profile |
| `ralph run --no-default-profiles --profile user:my-style` | 跳过 `profiles.default`,只使用 CLI 显式指定的 profile |
| `ralph inspect profiles -H builtin:debug --profile repo:strict` | 预览解析结果,不修改配置;支持 `--format human\|json` |

`ralph inspect profiles` 适合在第一次写完新 profile 目录后立刻验证:它会列出每个解析到的片段路径、首行预览(最多 60 字符)与所有 warnings(preset 子目录缺失、孤儿 hat、非 UTF-8 文件等)。

## 错误与警告

### 错误(命令立即失败)

- 显式请求的 profile(`--profile` 或 `profiles.default`)目录不存在——错误消息包含完整路径。
- profile 名为空 / 全空白 / 包含 `/` 或 `..`——拒绝并说明校验失败原因。
- `HOME` 未设置且 scope 是 `user`——清晰报错。
- `HatsSource::Remote` 与任意 active spec 组合——v1 不支持,返回清晰错误。
- 片段文件不是合法 UTF-8——返回 IO 错误,不 panic。

### 警告(命令继续,warning 写入 stderr)

- profile 目录存在,但缺少当前 preset 子目录(例如 `ralph-profiles/strict/` 没有 `debug/`)。
- profile 中有 `ghost.md`,但当前 preset 没有 `ghost` 这个 hat。
- `hats_source` 为 `None`(无 preset 名)但传了 `--profile`——只打 warning,不修改 config,不 panic。

## v1 范围

- 仅追加 `HatConfig.instructions`,不支持覆盖 backend / event_loop / topology。
- 仅精确匹配 profile / preset / hat-id;不支持通配符或正则。
- 不支持 profile 继承或嵌套,仅支持 ordered append。
- 不提供 `ralph profile create/init` 等脚手架命令。
- 不通过环境变量覆盖默认 `ralph-profiles/` 路径(后续迭代可能加入)。
- 不支持 `-c profiles.default=...` 形式的 CLI 覆盖;`profiles.default` 只能通过 `ralph.yml` 设置。
- 远程 hats source(`http://...`)与 profile 组合在 v1 不支持。

## 推荐用法

- 团队共享 → 用 `repo:` profile + 提交 `ralph-profiles/` 到 git。
- 个人偏好 → 用 `user:` profile 放在 `~/.config/ralph/profiles/`。
- CI / 临时任务 → 通过 `--profile repo:strict` 临时启用,无需修改 `ralph.yml`。
- 切分支测试 → 在 `ralph.yml` 中维护 `profiles.default` 列表,用 `--no-default-profiles` 临时绕开。
- 调试新 profile → 先 `ralph inspect profiles ...` 看解析结果,再 `ralph run ...` 真正启用。