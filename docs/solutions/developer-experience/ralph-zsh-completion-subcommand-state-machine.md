---
title: "Ralph zsh 补全：subcommand state machine 与 -H builtin: 零匹配"
date: 2026-07-24
category: developer-experience
module: ralph-cli
problem_type: logic_error
component: tooling
symptoms:
  - "`whence -w _ralph` 显示 function，但 `ralph run -H <TAB>` / `ralph run -H builtin:<TAB>` 无补全菜单"
  - "PTY 实测 `ralph run -H` 返回 nmatches=0；`ralph -H` 却弹出数百个 cwd 文件（do you wish to see all N possibilities?）"
root_cause: logic_error
resolution_type: code_fix
severity: high
tags: [zsh-completion, ralph-cli, builtin-hats, _arguments, oh-my-zsh, developer-experience]
---

# Ralph zsh 补全：subcommand state machine 与 -H builtin: 零匹配

## Problem

`_ralph` 已正确加载并注册到 `_comps[ralph]`，但 `ralph run -H` / `ralph run -H builtin:` 按 TAB 仍无可用补全。根因是补全函数内部的 `words`/`CURRENT` 状态机写错，叠加无条件 `_files` 把菜单淹成「看起来像 TAB 失效」。

## Symptoms

- `whence -w _ralph` → `function`，`_comps[ralph]=_ralph`
- `ralph run -H <TAB>` / `ralph run -H builtin:<TAB>`：无候选或无菜单
- 最小 `ZDOTDIR` PTY 实测：`ralph run -H` → `nmatches=0`；`ralph -H` → `nmatches≈410`（cwd 文件洪水）

## What Didn't Work

- 只修 `~/.zshrc` 的多重 `compinit` / 清 `zcompdump`：能让 `_ralph` 加载，但 TAB 仍无 `-H` 候选（加载问题 ≠ 补全逻辑问题）
- 假设 fzf 的 `^I` → `fzf-completion` 是主因：无 `**` 触发时会回落到 `expand-or-complete`，不是零匹配的根因
- 旧报告 `docs/solutions/ralph-zsh-completion-issue.md` 停在「compdef 已注册」层，未用真实 completion widget 观测 `nmatches`

## Solution

修改 `scripts/ralph-zsh-plugin.zsh`（并 `cp` 到 `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`）：

1. **`_ralph` 改用标准 `_arguments -C` 状态机**，让 `args` 状态下 `words[1]` 才是子命令：

```zsh
_arguments -C \
  $_ralph_main_opts \
  '1:ralph command:->cmds' \
  '*::arg:->args'

case $state in
  cmds) _describe 'ralph command' _ralph_cmds ;;
  args)
    case ${words[1]} in
      run|...) _ralph_subcmd_args ${words[1]} ;;
      ...
    esac
    ;;
esac
```

旧代码在 `CURRENT==2` 时用 `case ${words[1]} in run|...)`，但 zsh 的 `words[1]` 是 `ralph`，永远匹配不到 `run`，因此 `_ralph_run_args`（含 `-H`）从未被调用。

2. **`compadd -Q`**：`builtin:name` 含冒号，避免被当成 completion separator。

3. **`_ralph_hat_source` 仅在路径型 PREFIX 时才 `_files`**，避免空 `-H` 被数百个 cwd 文件淹没：

```zsh
_ralph_builtin_hats && ret=0
if [[ $PREFIX == .* || $PREFIX == /* || $PREFIX == ~* || $PREFIX == */* ]]; then
  _files && ret=0
fi
```

## Why This Works

- `#compdef ralph` 下 `words[1]` 始终是命令名；子命令补全必须用 `_arguments -C` + `*::arg:->args` 重写 `words`，不能手写 `CURRENT`/`words[1]` 开关。
- `-H` 的 action 在 `_ralph_run_args` 里；进不了该函数就必然 `nmatches=0`。
- 无条件 `_files` 会触发 zsh 的 “do you wish to see all N possibilities?”，交互上等同于补全失效。

## Prevention

- 改 `scripts/ralph-zsh-plugin.zsh` 后必须同步安装：`cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`，再 `exec zsh`。
- 用最小 `ZDOTDIR` + PTY 断言 `nmatches`（不要只查 `whence -w _ralph`）：
  - `ralph run -H ` → 期望 7（builtin 数）
  - `ralph run -H builtin:` → 期望 7
- 新增/删除 public builtin preset 时同步 `_RALPH_BUILTIN_HAT_VALUES`（见 `docs/solutions/developer-experience/ralph-zsh-builtin-hat-completion-maintenance-2026-05-26.md`）。

## Related Issues

- `docs/solutions/ralph-zsh-completion-issue.md` — 同症状的早期未闭合诊断（加载顺序层）
- `docs/solutions/developer-experience/ralph-zsh-builtin-hat-completion-maintenance-2026-05-26.md` — builtin 列表与 `compadd`/`:` 维护约定
