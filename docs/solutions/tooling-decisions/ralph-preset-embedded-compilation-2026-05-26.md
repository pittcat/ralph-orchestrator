---
title: Ralph preset 内嵌编译机制
date: 2026-05-26
category: tooling-decisions
module: ralph-cli
problem_type: tooling_decision
component: tooling
severity: medium
applies_when:
  - 理解为何 builtin preset 需要 presets/ 和 crates/ralph-cli/presets/ 之间的同步
  - 向 Ralph CLI 添加新的 builtin preset
  - 调试 builtin preset 的 "Unknown hat collection" 错误
tags: [preset, embedded-compilation, include-str, sync-script, builtin-hats, tooling]
---

# Ralph preset 内嵌编译机制

## 背景

用户可能会将 preset YAML 文件添加到 `presets/`（仓库根目录中 tab 补全可见的目录），期望它们能配合 `ralph -H builtin:<name>` 使用。然而 builtin preset 系统通过 `presets.rs` 中的 `include_str!` 在编译时将 preset 编译进二进制文件。如果某个 preset 存在于 `presets/` 但从未同步到 `crates/ralph-cli/presets/`，它就不会在编译时被内嵌，CLI 会在请求该 preset 时报告 "Unknown hat collection"。

## 指导

Ralph 的 builtin preset 系统采用**双目录架构**，包含一个规范源和一个编译时的镜像。

### `include_str!` 模式

在 `crates/ralph-cli/src/presets.rs` 中，每个内嵌 preset 通过以下方式声明：

```rust
EmbeddedPreset {
    name: "code-assist",
    description: "Default implementation workflow with TDD and adversarial validation",
    content: include_str!("../presets/code-assist.yml"),
    public: true,
},
```

路径 `../presets/` 相对于 `crates/ralph-cli/src/`，解析为 `crates/ralph-cli/presets/` —— 这正是镜像必须存在的位置，以便 `include_str!` 在编译时找到文件。`include_str!` 直接将文件内容嵌入二进制的只读数据段；文件必须在构建时存在。

### 同步脚本

`scripts/sync-embedded-files.sh` 负责维护镜像。其 `MIRRORED_FILES` 数组将规范源映射到它们的编译时目标：

```bash
MIRRORED_FILES=(
    "presets/autoresearch.yml:crates/ralph-cli/presets/autoresearch.yml"
    "presets/code-assist.yml:crates/ralph-cli/presets/code-assist.yml"
    "presets/debug.yml:crates/ralph-cli/presets/debug.yml"
    ...
)
```

不带参数运行以同步，或使用 `check` 进行 CI 风格验证：

```bash
./scripts/sync-embedded-files.sh        # 同步所有镜像文件
./scripts/sync-embedded-files.sh check  # CI 检查同步状态
```

## 为什么这很重要

`include_str!` 要求文件在编译时（而非运行时）存在于 crate 目录中。发布到 crates.io 时，crate 被打包并独立提取 —— crate 目录之外的文件（如仓库根目录的 `presets/`）不会被包含。`crates/ralph-cli/presets/` 中的镜像才是实际被内嵌的内容。

规范源和镜像必须保持同步，原因如下：

- **将 preset 添加到 `presets/` 但不同步** → `presets.rs` 中的编译时查找无法找到文件 → 构建错误或二进制中静默遗漏
- **修改 preset 但不同步** → 二进制嵌入的是镜像中的旧内容，而非新的规范内容

## 何时适用

- **添加新的 builtin preset**：首先在 `presets/` 创建 YAML 文件，然后运行同步脚本将其复制到 `crates/ralph-cli/presets/`，最后在 `presets.rs` 中添加 `include_str!` 条目
- **修改现有的 builtin preset**：在 `presets/` 中编辑规范文件，然后运行同步脚本更新镜像
- **拉取更改后**：如果协作者添加或修改了 preset，在构建前运行同步脚本
- **发布或构建 release 前**：运行 `sync-embedded-files.sh check` 以捕获漂移

## 示例

### 同步操作（幕后发生了什么）

```bash
# 对于 MIRRORED_FILES 中的每个映射：
src="presets/code-assist.yml"
dest="crates/ralph-cli/presets/code-assist.yml"
cp "$REPO_ROOT/$src" "$REPO_ROOT/$dest"
```

### 检查操作（CI 验证）

```bash
# 检测漂移，如果任何镜像过时或缺失则退出 1：
./scripts/sync-embedded-files.sh check
# 输出：
# ✓ crates/ralph-cli/presets/code-assist.yml
# OUT OF SYNC: crates/ralph-cli/presets/debug.yml
#   Source: presets/debug.yml
#   Diff: ...
# ERROR: Embedded assets are out of sync!
```

### `presets.rs` 中最终的 `include_str!` 条目

```rust
EmbeddedPreset {
    name: "code-assist",
    content: include_str!("../presets/code-assist.yml"),  // 解析为 crates/ralph-cli/presets/code-assist.yml
    ..
},
```

### preset 未同步时的错误

```
$ ralph -H builtin:my-new-preset
Error: Unknown hat collection "builtin:my-new-preset"
Available: autoresearch, code-assist, debug, merge-loop, pdd-to-code-assist, research, review
```

发生此错误是因为 `get_preset("my-new-preset")` 返回 `None` —— 该 preset 从未被添加到 `presets.rs` 的 `PRESETS` 数组中，而该数组只包含 `include_str!` 路径指向 `crates/ralph-cli/presets/` 中存在文件的条目。

## 相关

- `docs/solutions/developer-experience/ralph-zsh-builtin-hat-completion-maintenance-2026-05-26.md` — builtin hat collection 维护规则（zsh 补全与 preset 同步）
- `crates/ralph-cli/src/presets.rs` — 内嵌 preset 实现
- `scripts/ralph-zsh-plugin.zsh` — zsh 补全脚本