---
date: 2026-06-25
topic: profiles-for-preset-role-tuning
---

# Profiles for Preset Role Tuning

## Problem Frame

Ralph 的 builtin preset 是编译进二进制的 YAML，运行时通过 `-H` 或 `-c` 做配置层叠加。但用户如果想让同一个 preset 在不同项目、不同场景下表现出不同风格（例如严格验证 vs. 快速原型 vs. 中文输出），当前只能 fork 出多份 preset 文件，或把偏好塞进 `ralph.yml` 的 `hats.<id>.extra_instructions`。

这带来两个问题：

1. **维护负担重**：base preset 升级时，所有 fork 版本都要同步修改。
2. **共享与隔离不清**：repo 级偏好和个人偏好混在一起，没有明确的文件边界。

Profile 机制的目标是让 preset 保持通用，同时通过一组小型的 markdown 片段在运行时追加到特定 hat 的 instructions 末尾，实现「同一个 preset，多种风格」。

---

## Actors

- A1. **Operator（人类用户）**：在项目里运行 `ralph run`，根据场景选择不同 profile。
- A2. **Team member**：通过 git 共享 repo-scoped profile，统一团队风格。
- A3. **Agent / CI**：通过 `--profile` 或 `profiles.default` 稳定复现某套风格。

---

## Key Flows

- F1. **CLI 显式激活**
  - **Trigger:** 用户在项目根目录执行 `ralph run ce-executor-serial --profile repo:strict --profile user:my-style "..."`。
  - **Actors:** A1
  - **Steps:**
    1. CLI 解析 `--profile` 列表，校验 scope 为 `repo` 或 `user`。
    2. 加载 builtin preset `ce-executor-serial`，解析为 `RalphConfig`。
    3. 对每一个 active profile，按顺序解析 `<profile-dir>/ce-executor-serial/<hat-id>.md`。
    4. 把每个 fragment 追加到对应 hat 的 `instructions` 末尾。
    5. 使用增强后的配置启动 event loop。
  - **Outcome:** Loop 运行时，backend 收到的 prompt 包含 preset 原 instructions + profile 追加片段。
  - **Covered by:** R1, R2, R3, R6, R10, R11

- F2. **Config 默认激活**
  - **Trigger:** 项目 `ralph.yml` 声明了 `profiles.default`。
  - **Actors:** A1, A3
  - **Steps:**
    1. 用户执行 `ralph run ce-executor-serial "..."`（无 `--profile`）。
    2. 运行时读取 `profiles.default` 列表作为默认 active profiles。
    3. 用户可通过 `--no-default-profiles` 关闭默认列表，仅保留显式 `--profile`。
  - **Outcome:** 不敲 flag 也能自动应用项目约定的风格。
  - **Covered by:** R4, R5

- F3. **团队共享 repo profile**
  - **Trigger:** 新成员 clone 仓库，目录里已存在 `ralph-profiles/strict/`。
  - **Actors:** A2
  - **Steps:**
    1. `ralph-profiles/` 作为普通目录提交到 git（不被 `.ralph/` gitignore 影响）。
    2. 新成员直接跑 `ralph run ce-executor-serial --profile repo:strict` 即可使用团队风格。
  - **Outcome:** 团队偏好随代码库一起版本化。
  - **Covered by:** R2

---

## Requirements

**激活与作用域**

- R1. `ralph run` 支持 `--profile <scope>:<name>` flag，可重复指定；profile 按给定顺序生效。
- R2. `repo:<name>` 解析为 `<project-root>/ralph-profiles/<name>/`。
- R3. `user:<name>` 解析为 `~/.config/ralph/profiles/<name>/`；若存在 `$XDG_CONFIG_HOME`，则使用 `$XDG_CONFIG_HOME/ralph/profiles/`。
- R4. `ralph.yml` 支持 `profiles.default` 字段，值为逗号分隔的 profile spec 列表（例如 `"repo:strict, user:my-style"`）。
- R5. `ralph run` 支持 `--no-default-profiles`，仅关闭 config 默认 profile，不影响显式 `--profile`。

**片段解析规则**

- R6. 设当前 active preset 名为 `P`，则从每个 active profile 目录加载 `<profile-dir>/<P>/<hat-id>.md`。
- R7. 只加载 `.md` 文件；片段对应 hat 不在当前 preset 中时，发出 warning 并忽略。
- R8. 显式请求的 profile 目录不存在时，立即报错并给出清晰路径提示。
- R9. profile 存在但缺少当前 preset 子目录时，发出 warning 并跳过该 profile 的本次贡献。
- R10. 多个 profile 按 activation order 追加片段：config defaults 在前，CLI `--profile` 在后；同一 hat 的片段按顺序拼接。

**Prompt 组合**

- R11. 每个片段以换行分隔追加到对应 hat 的 `instructions` 字段末尾，即：`<original instructions>\n<fragment-1>\n<fragment-2>`。
- R12. Profile 片段在 `RalphConfig.normalize()` 之后、运行时模板变量展开（如 `STATE_DIR`）之前生效。

**可观测性**

- R13. `ralph inspect profiles`（或扩展现有 `ralph inspect` 子命令）可展示：当前 active profiles、config defaults、解析到的片段路径、每段首行预览、warnings。

**集成边界**

- R14. Profile 应用在 preset / operator config / `-H` hat overlay / `-c` CLI override 全部合并完成、且 `normalize()` 执行完毕之后；但在 event loop 消费 hat instructions 之前。
- R15. Profile 只修改 hat instructions，不修改 topology、event 契约、backend 配置、event_loop 配置、hat 的 `publishes`/`subscribes` 等结构。

---

## Acceptance Examples

- AE1. **Covers R1, R2, R6, R11.** 项目 `my-rust-repo` 存在 `ralph-profiles/strict/ce-executor-serial/executor.md`，内容为 `- Run cargo nextest run after every change.`。执行 `ralph run ce-executor-serial --profile repo:strict "fix race"` 后，`executor` hat 的 instructions 末尾追加该片段。

- AE2. **Covers R4, R5, R10.** `ralph.yml` 中 `profiles.default = "repo:base-style"`，同时执行 `ralph run ce-executor-serial --profile user:my-style "..."`。激活顺序为 `repo:base-style` 先、`user:my-style` 后；若两者都有 `executor.md`，则 `user:my-style` 的片段追加在后。若执行 `--no-default-profiles --profile user:my-style`，则仅 `user:my-style` 生效。

- AE3. **Covers R8, R9, R7.** 执行 `ralph run ce-executor-serial --profile repo:missing` 时，若 `ralph-profiles/missing/` 不存在，直接报错。若存在 `ralph-profiles/old/` 但没有 `old/ce-executor-serial/` 子目录，则 warn 并继续。若存在 `old/ce-executor-serial/ghost.md` 但 preset 里没有 `ghost` hat，则 warn 并忽略该文件。

---

## Success Criteria

- 用户维护一份 builtin preset，就能通过 profile 在 strict / hack / zh-cn 等风格间切换，无需 fork preset YAML。
- Repo-scoped profile 可通过 git 提交共享，不被 `.ralph/` gitignore 影响。
- 激活顺序、路径解析、错误/警告行为与 autoloop 的 profile 语义一致，降低认知成本。
- 实现完成后，`cargo nextest run -p ralph-cli --bin ralph -- profile` 相关测试通过，且 `./scripts/run-tests.sh` 全量通过。

---

## Scope Boundaries

- v1 仅支持 hat instructions 的追加，不支持通过 profile 覆盖 backend、event_loop、topology 等结构。
- v1 仅支持精确匹配：`profile` 名、`preset` 名、`hat-id` 全等，不支持通配符或正则。
- v1 不支持 profile 之间的继承或嵌套，仅支持 ordered append。
- v1 不提供 `ralph profile create/init` 等 scaffold 命令；profile 目录由用户手动创建。
- 不在本需求范围内：用 profile 替换 preset、修改 event 拓扑、修改 hat 的 `publishes`/`subscribes`、运行时动态切换 profile。

---

## Key Decisions

- **Repo profile 路径不使用 `.ralph/profiles/`**：`.ralph/` 是 ralph 运行时状态目录，被 `.gitignore` 排除；repo profile 必须能被 git 追踪，因此放在项目根目录的 `ralph-profiles/`。
- **Markdown 片段在运行时从磁盘加载，不编译进二进制**：builtin preset YAML 仍通过 `include_str!` 编译打包；profile 作为运行时叠加层，在内存中与 preset 融合。
- **激活语法沿用 autoloop 约定**：`repo:<name>` / `user:<name>`，避免 bare name 的 scope 歧义。
- **片段追加在 config normalize 之后、模板变量展开之前**：保证 profile 追加的是「用户可见 instructions」，同时仍能被 `STATE_DIR` 等模板变量处理。

---

## Dependencies / Assumptions

- 运行时已经知道当前 active preset 的名字（builtin 名或 `-H` 文件 preset 的识别名）。
- 解析 repo profile 时需要项目根目录（workDir）的绝对路径。
- CLI 的 clap parser 需要新增 `--profile` 和 `--no-default-profiles` flag。
- `RalphConfig` 需要新增可选的 `profiles` 配置块（至少包含 `default` 字段）。
- 本功能不改动现有 preset YAML 的 event 拓扑，因此不触发 preset/schema 改动后的 7 步同步清单。

---

## Outstanding Questions

### Deferred to Planning

- [影响 R13][UX] `ralph inspect profiles` 的具体输出格式和命令层级：是新增子命令还是扩展现有 `ralph inspect`？
- [影响 R7, R9][UX] profile warnings 是在普通运行时 stderr 输出，还是仅在 `--verbose` / `inspect` 时输出？
- [影响 R6][技术] 当 active preset 来自 `-H ./my-hats.yml` 文件时，profile 子目录名应使用什么 identifier（文件名 basename、还是要求 preset 显式命名）？
- [影响 R2][技术] 是否需要支持通过 env var（如 `$RALPH_REPO_PROFILES_DIR`）覆盖默认 `ralph-profiles/` 路径？

---

## Next Steps

- `-> /ce-plan` 进入结构化实现规划。
