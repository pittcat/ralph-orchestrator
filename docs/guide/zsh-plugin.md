# Zsh 插件安装指南

Ralph 提供了 zsh 插件，提供命令补全和别名功能。

## 安装方式

### 方式一：通过 oh-my-zsh（推荐）

1. 确保 oh-my-zsh 已安装
2. 将插件复制到 oh-my-zsh 插件目录：

```bash
mkdir -p ~/.oh-my-zsh/plugins/ralph
cp /path/to/ralph-orchestrator/scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh
```

3. 在 `~/.zshrc` 中添加插件到列表：

```zsh
plugins=(... ralph)
```

4. 重新加载 zsh 配置：

```bash
source ~/.zshrc
```

### 方式二：通过 zsh 插件目录

如果你使用自定义插件管理工具（如 zinit、antigen 等）：

```zsh
# 例如使用 zinit
zinit load /path/to/ralph-orchestrator/scripts/ralph-zsh-plugin.zsh

# 或使用 antigen
antigen bundles /path/to/ralph-orchestrator/scripts/ralph-zsh-plugin.zsh
```

### 方式三：直接 source

在 `~/.zshrc` 中直接添加：

```zsh
source /path/to/ralph-orchestrator/scripts/ralph-zsh-plugin.zsh
```

## 功能

- **命令补全**：支持 `ralph` 命令及其子命令的 tab 补全
- **内置 hat 补全**：提供 builtin hat 名称的补全
- **后端补全**：支持选择不同 backend 的补全
- **别名**：提供常用命令的快捷别名

## 验证安装

```zsh
ralph <TAB>
```

应该能看到 `run`、`preflight`、`hooks`、`doctor`、`tutorial`、`events`、`init`、`clean` 等子命令的补全。