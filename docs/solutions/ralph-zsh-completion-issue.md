---
title: "Ralph Zsh 插件补全问题报告"
date: 2026-06-09
category: ralph-cli
tags: [cli, zsh, completion, bug]
---

# Ralph Zsh 插件补全问题报告

## 问题描述

用户反馈: `ralph run -H builtin:` 按 TAB 没有任何补全产生。

## 已尝试的修复

### 修复 1: 调整 zshrc 加载顺序

**文件**: `~/.mac-dotfile/zsh/zshrc` (通过 symlink 指向 `~/.zshrc`)

**改动**: 将 ralph 插件从 `compinit` 之前移到之后

```zsh
# 之前 (错误)
source "$HOME/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh"  # 第 102 行
autoload -Uz compinit && compinit                         # 第 106 行

# 之后 (正确)
fpath=(/Users/pittcat/.zsh/completions $fpath)
autoload -Uz compinit && compinit
source "$HOME/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh"
```

### 修复 2: 移除 hat_collection state 中的 `_files -/`

**文件**: `scripts/ralph-zsh-plugin.zsh` (两处)

**问题**: 在 `_describe 'hat source' _RALPH_BUILTIN_HATS` 之后调用了 `_files -/`，这会干扰补全结果

```zsh
# 之前 (有问题)
hat_collection)
  _describe 'hat source' _RALPH_BUILTIN_HATS
  _files -/    # <-- 这行干扰了补全
  ;;

# 之后 (正确)
hat_collection)
  _describe 'hat source' _RALPH_BUILTIN_HATS
  ;;
```

### 修复 3: 同步 oh-my-zsh 插件目录

```bash
cp /Users/pittcat/Dev/Rust/ralph-orchestrator/scripts/ralph-zsh-plugin.zsh \
   ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh
```

## 当前状态

**修复后仍然无效** — 用户执行 `source ~/.zshrc` 后补全仍然不工作。

## 深入诊断结果

### 诊断 1: compdef 确实成功注册

```zsh
$ compdef _ralph ralph
$ echo $?
0  # 成功

$ echo ${_comps[ralph]}
_ralph  # 确认已写入 _comps 关联数组
```

**结论**: compdef 工作正常，`_comps[ralph] = _ralph` 已正确设置。

### 诊断 2: _ralph 函数能正确设置 hat_collection state

手动模拟 `_ralph_run_args` 逻辑，当 `CURRENT=4` (即 `-H builtin:` 位置) 时：
- `_arguments -C $run_opts` 正确设置 `state=hat_collection`
- 随后 `_describe 'hat source' _RALPH_BUILTIN_HATS` 应该提供补全

**结论**: 补全逻辑本身没问题，state 和数组都正确定义。

### 诊断 3: 补全系统完整链路测试

在交互式 zsh 中测试时：
- `_comps[ralph] = _ralph` ✓
- `_ralph` 函数存在 ✓
- `compdef _ralph ralph` 返回成功 ✓
- `words` 和 `CURRENT` 变量在 TAB 时会被 zsh 正确设置

但 `BUFFER` 和 `CURSOR` 无法在测试脚本中模拟真实按键事件。

**结论**: 代码链路完整，但非交互式环境无法验证实际按键响应。

### 诊断 4: oh-my-zsh 插件安装方式

```
~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh  # 存在
```

有两种加载方式：
1. **直接 source** (当前配置): `source "$HOME/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh"`
2. **oh-my-zsh 插件机制**: 在 `plugins=(...)` 中添加 `ralph`

两种方式都调用相同的 `compdef _ralph ralph`。

## 待排查方向

### 1. kaku shell integration 可能干扰

zshrc 中有:
```zsh
[[ -f "$HOME/.config/kaku/zsh/kaku.zsh" ]] && source "$HOME/.config/kaku/zsh/kaku.zsh"
```

kaku 可能在之后重新配置了补全系统或修改了 `$fpath`。

### 2. 需要用户手动验证

```zsh
# 1. 确认插件已加载且 compdef 成功
whence -f _ralph && echo "_ralph found" || echo "_ralph NOT found"

# 2. 确认补全已注册
echo ${_comps[ralph]:-NOT REGISTERED}

# 3. 重启 zsh 后测试
exec zsh
ralph run -H builtin:<TAB>
```

### 3. 可能需要在真实终端中测试

非交互式 `zsh -i -c` 测试无法完整模拟 TAB 按键触发的补全流程。建议用户在真实终端中测试并观察结果。

## 下一步

1. 用户需要在新的终端窗口中测试 `ralph run -H builtin:<TAB>`
2. 如果仍无效，检查 kaku 是否在之后修改了补全配置
3. 尝试将 ralph 作为 oh-my-zsh 插件加载（通过 `plugins=(...)` 而非直接 source）