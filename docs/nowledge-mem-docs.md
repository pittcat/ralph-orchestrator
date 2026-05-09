# Nowledge Mem 完整文档

> **来源**: [https://mem.nowledge.co/zh/docs](https://mem.nowledge.co/zh/docs)  
> **整理日期**: 2026-04-27  
> **说明**: 本文档为 Nowledge Mem 官方文档的完整离线整理版，涵盖产品介绍、安装配置、核心功能、工具集成、使用场景、技术概念、部署指南和参考文档等全部内容。

---

## 目录

### 第一部分：入门指南
- [Nowledge Mem 文档首页](#nowledge-mem-文档首页)
- [Nowledge Mem 产品介绍](#nowledge-mem-产品介绍)
- [从这里开始](#从这里开始)
- [安装](#安装)
- [快速入门](#快速入门)
- [导入已有对话](#导入已有对话)
- [如何确认 Mem 已经在工作](#如何确认-mem-已经在工作)

### 第二部分：核心功能
- [使用 Nowledge Mem](#使用-nowledge-mem)
- [记忆](#记忆)
- [对话](#对话)
- [资料库](#资料库)
- [Spaces](#spaces)
- [AI Now](#ai-now)
- [后台智能](#后台智能)
- [你的档案](#你的档案)

### 第三部分：工具集成（总览与主要工具）
- [集成总览](#集成总览)
- [安全地自定义集成行为](#安全地自定义集成行为)
- [浏览器扩展](#浏览器扩展)
- [Claude Code](#claude-code)
- [Claude Desktop](#claude-desktop)
- [Droid](#droid)
- [Cursor](#cursor)

### 第四部分：更多工具集成
- [Codex CLI](#codex-cli)
- [Copilot CLI](#copilot-cli)
- [Gemini CLI](#gemini-cli)
- [Alma](#alma)
- [OpenClaw](#openclaw)
- [Bub](#bub)
- [OpenCode](#opencode)

### 第五部分：其他集成与使用场景
- [Pi](#pi)
- [Hermes Agent](#hermes-agent)
- [Raycast](#raycast)
- [随处访问 Mem](#随处访问-mem)
- [多设备同步](#多设备同步)
- [使用场景概述](#使用场景概述)
- [你的知识，你做主](#你的知识你做主)
- [永不丢失会话](#永不丢失会话)

### 第六部分：进阶使用场景与核心概念
- [穿越时间搜索](#穿越时间搜索)
- [你的笔记，无处不在](#你的笔记无处不在)
- [看见你的专长](#看见你的专长)
- [运作原理](#运作原理)
- [知识演化](#知识演化)
- [搜索架构](#搜索架构)
- [记忆衰减](#记忆衰减)
- [后台智能概念](#后台智能概念)

### 第七部分：部署与参考
- [Linux 服务器部署](#linux-服务器部署)
- [LLM 提供商](#llm-提供商)
- [搜索与相关性](#搜索与相关性)
- [Nowledge Mem CLI](#nowledge-mem-cli)
- [Browse Now](#browse-now)
- [API Reference](#api-reference)
- [故障排除](#故障排除)
- [Mem Pro 计划](#mem-pro-计划)

---


---

# 第一部分：入门指南

# Nowledge Mem 文档

> 来源：https://mem.nowledge.co/zh/docs

---

# Nowledge Mem

## 中立的知识层，让你自由切换任何 AI 工具

你的 AI 工具不会替你记住已经做过的工作。Nowledge Mem 会。

它是面向 AI 工作的个人知识层。你保存决策、洞察、资料或对话，之后可以搜索回来、和其他内容关联起来，也可以让你接入的工具直接从同一份上下文继续。

你不需要一开始就把所有东西都接好。先保存一条内容，把它找回来，再让一个真实工具用上它。只要亲眼看到这一步发生，你就会很快明白 Mem 到底有什么用。

如果你想在多台设备之间使用 Mem，现在也已经可以：让一台常开机器运行主 Mem，再让桌面端、网页端、移动端和受支持工具连接到同一个知识库。详见[多设备同步](https://mem.nowledge.co/zh/docs/sync)。

如果你是第一次使用，最短路径就是：

- [先看从这里开始](https://mem.nowledge.co/zh/docs/start-here)
- [先完成安装](https://mem.nowledge.co/zh/docs/installation)
- [保存第一条记忆](https://mem.nowledge.co/zh/docs/getting-started)
- [确认 Mem 真的已经在工作](https://mem.nowledge.co/zh/docs/verify-it-works)

---

### 从这里开始

给新用户的最短起步方式：先保存一条记忆，选一条工具路径，再确认它真的已经工作。

### 用任何工具，不丢上下文

先把一个真实工具接进来。之后，同一套记忆就能在 Claude Code、Cursor、Codex、ChatGPT 等工具之间继续被使用。

### 它在你睡觉时学习

系统会在后台把相关想法连起来、标记矛盾，并在早上写一份简报给你的 AI 工具参考。

### 一切互联

你的记忆通过共享实体形成图谱。可以按语义搜索、可视化浏览，也能让你看到自己不会主动去找的关联。

---

### 选择你的第一条路径

不要一次把所有东西都配上。先按你最常用的工具来选择路径。只有在没有专属路径时，再考虑复用包或直接 MCP。

| 工具 | 说明 |
|------|------|
| [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code) | 原生插件，自动读取简报、检索知识并保存会话 |
| [Cursor](https://mem.nowledge.co/zh/docs/integrations/cursor) | 原生插件，带来工作记忆简报、路由式检索与清晰的交接摘要语义 |
| Gemini CLI | 原生扩展，带命令、生命周期钩子和真实会话保存 |
| Codex CLI | 复用型工作流包，结合提示词、AGENTS 指导与真实会话保存 |
| Copilot CLI | 原生插件，提供 Working Memory 启动、检索引导与会话捕获 |
| [OpenClaw（5 分钟）](https://mem.nowledge.co/zh/docs/integrations/openclaw) | OpenClaw 原生插件的首次配置指南 |
| [Claude Desktop](https://mem.nowledge.co/zh/docs/integrations/claude-desktop) | 一键安装扩展，支持搜索、保存和更新记忆 |
| [浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension) | 从受支持的 Web AI 聊天平台捕获对话 |

### 导入你的文档

拖入 PDF、Word 文档或演示文稿，自动解析并与记忆一起索引。在 Timeline 中提问时，答案同时来自文档和记忆。

### 本地优先隐私

Nowledge Mem 是本地优先的。默认情况下，你的知识都保留在自己的设备上。需要更强处理能力时可以连接远程 LLM，但你的数据不会经过 Nowledge 服务器。

---

### 文档导航

- [从这里开始](https://mem.nowledge.co/zh/docs/start-here)：安装后最该先看的页面
- [安装](https://mem.nowledge.co/zh/docs/installation)：几分钟内上手
- [快速入门](https://mem.nowledge.co/zh/docs/getting-started)：你的前五分钟
- [如何确认 Mem 已经在工作](https://mem.nowledge.co/zh/docs/verify-it-works)：快速验证搜索、捕获和工具连接

---


---

# 从这里开始

> 来源：https://mem.nowledge.co/zh/docs/start-here

如果你刚装好 Nowledge Mem，想尽快知道该先做什么，就从这里开始。

理解 Nowledge Mem，最好的方式是先走一遍：存一条真实内容，把它找回来，再让一个工具用上它。

如果你刚装好应用，想尽快知道怎么开始，就从这里开始。

---

## Mem 到底是什么

简单说，Mem 帮你把决定、洞察、文件和对话存下来，让它们之后还能被搜索和复用，也让你连接的工具不用每次从零开始。

如果你打算在多台设备之间使用 Mem，也不用再猜它支不支持同步。现在的方式很明确：一台常开设备运行主 Mem，其他客户端连接到它。详见[多设备同步](https://mem.nowledge.co/zh/docs/sync)。

你不需要先把整个产品研究明白，才开始得到帮助。

---

## 先完成第一轮使用

### 1. 打开应用

先完成[安装](https://mem.nowledge.co/zh/docs/installation)，启动应用，走完首次启动流程。

### 2. 保存一条真实记忆

在 Timeline 里，写下一条你真的想留住的内容：

- 一个你已经做出的决定
- 一条工作中的有用洞察
- 一个你经常重复的个人偏好

然后按回车。

### 3. 确认搜索能把它找回来

直接在 Timeline 里问一句：

> 我之前对认证方案做过什么决定？

如果答案能反映你刚刚保存的内容，说明搜索已经正常工作了。

### 4. 再连接一个真实工具

先选一条最符合你当前主力工具的路径。

---

## 只选一条路径

### 先弄清楚"最好的效果"是什么意思

很多新用户会默认以为，每条接入路径带来的自主能力都一样。其实不是。

可以这样理解：

| 路径 | 你通常会得到什么 | 想拿到最好效果，还需要什么 |
|------|-----------------|---------------------------|
| **原生集成** | 最强路径。通常会自动完成会话启动时的上下文加载，有些宿主还会自动捕获线程，或通过生命周期钩子驱动记忆行为。 | 安装原生包；如果指南要求，就保证本机有 `nmem`；然后重启宿主。 |
| **复用型工作流包** | Working Memory、检索、提炼这些行为是通过 skills、rules 或 prompts 教出来的。 | 安装该包；保证本机可用 `nmem`；如果指南建议，再补上项目级 `AGENTS.md` 等行为说明。 |
| **直接 MCP** | 工具本身可用，但宿主是否会主动、稳定地调用它们，还取决于你有没有给它合适的系统提示或规则。 | 配好 MCP server，再加上推荐的系统提示或 `AGENTS.md` 片段。 |

如果你的工具已经有专属的 Nowledge 集成，就先走那条路径。那通常是最短、也最稳的办法。

---

### 1. 我想先直接用 Mem 本身

先从应用本体开始：

- [快速入门](https://mem.nowledge.co/zh/docs/getting-started)：了解 Timeline
- [AI Now](https://mem.nowledge.co/zh/docs/ai-now)：当你想要一个建立在知识之上的个人 AI 工作区时

如果你想先熟悉 Mem 本身，再去连接外部工具，这是最稳妥的路径。

### 2. 我平时主要用 ChatGPT、Claude 或 Gemini 网页版

安装[浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension)。

你的第一个成功状态可以非常简单：

1. 打开侧边栏
2. 如果需要，先把它连接到 Mem
3. 捕获或提炼一段对话
4. 在 Mem 里看到对应的记忆或线程

### 3. 我主要用编程助手

直接进入你已经在用的工具对应的专属指南：

- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)
- [Cursor](https://mem.nowledge.co/zh/docs/integrations/cursor)
- [Gemini CLI](https://mem.nowledge.co/zh/docs/integrations/gemini-cli)
- [Copilot CLI](https://mem.nowledge.co/zh/docs/integrations/copilot-cli)
- [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw)
- [Alma](https://mem.nowledge.co/zh/docs/integrations/alma)
- [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli)

如果这个工具已经有专属的 Nowledge 集成，就先用它。不要一上来就走通用 MCP 或 CLI，除非那份指南明确让你这么做。

### 4. 我已经有导出包或本地会话要迁进来

若你上手 Mem 时手里已有 ZIP/JSON/HTML 导出（ChatGPT、DeepSeek、ChatWise、Alma）、本机编程助手会话，或单条 `.md` 对话，请从[导入已有对话](https://mem.nowledge.co/zh/docs/import-existing-conversations)开始。它会说明每种情况对应 Mem 里的哪个界面，不用在「对话」和「集成」之间猜。

### 5. 我有自定义智能体或终端工作流

只有在前面几条都不适合时，再走这条路径：

- [集成](https://mem.nowledge.co/zh/docs/integrations)：了解复用包与 MCP
- [CLI](https://mem.nowledge.co/zh/docs/cli)：使用 `nmem`

这是进阶路径，不是默认路径。

---

## 第一天先少做一点

不要把 MCP、`npx skills`、CLI、浏览器捕获和 AI Now 一次全配上。先把一个真实场景真正用起来，再往外扩展。

---

## 第一轮成功清单

- [ ] 我已经在 Timeline 里保存了一条记忆
- [ ] 我能把它重新搜出来
- [ ] 我已经为自己真正用的工具选定了一条主路径
- [ ] 我已经看到一条记忆、一条线程，或一个明确来自我自己知识的回答

如果你还缺其中任何一项，请继续看[如何确认 Mem 已经在工作](https://mem.nowledge.co/zh/docs/verify-it-works)。

---

## 最短起步顺序

对大多数新用户来说，最好的顺序就是：打开应用，保存一条记忆，确认搜索能找回来，然后只连接一个工具。

---

## 下一步

- [安装](https://mem.nowledge.co/zh/docs/installation)：完成安装与首次启动
- [快速入门](https://mem.nowledge.co/zh/docs/getting-started)：了解 Timeline 和第一次输入
- [如何确认 Mem 已经在工作](https://mem.nowledge.co/zh/docs/verify-it-works)：验证 Mem 是否真的已经接通
- [集成](https://mem.nowledge.co/zh/docs/integrations)：为你的工具选择正确的连接路径
- [随处访问](https://mem.nowledge.co/zh/docs/remote-access)：从任何设备使用 Mem——在一台机器上运行，从所有设备连接
- [多设备同步](https://mem.nowledge.co/zh/docs/sync)：理解"一台 Mem，多端接入"的同步模型

---


---

# 安装

> 来源：https://mem.nowledge.co/zh/docs/installation

在 macOS、Windows 或 Linux 上安装 Nowledge Mem

---

## 私有 Alpha 测试访问

Nowledge Mem 目前处于**私有 Alpha 测试**阶段。获取下载链接：

- **登录**：前往 [mem.nowledge.co/licenses](https://mem.nowledge.co/licenses) 获取下载链接
- 还没有访问权限？[申请 Alpha 测试](https://nowled.ge/alpha)，或前往[定价](https://mem.nowledge.co/zh/pricing)完成开通

---

## 安装后先做什么

不要一上来把所有东西都配好。先看[从这里开始](https://mem.nowledge.co/zh/docs/start-here)，在 Timeline 保存一条记忆，然后只为你真正使用的工具选择一条连接路径。

---

## 系统要求

最低系统要求：

| 要求 | 规格 |
|------|------|
| **操作系统** | macOS 15 或更高版本；Windows 10 或更高版本；Linux x86_64（Ubuntu 22.04+、Debian 12+，其他发行版可使用 AppImage） |
| **内存 (RAM)** | 最低 16 GiB |
| **磁盘空间** | 10 GiB 可用空间 |
| **网络** | 如果使用网络代理，请确保绕过 `127.0.0.1` 和 `localhost` |

**Linux 无头服务器**同样受支持。参阅 [Linux 服务器部署](https://mem.nowledge.co/zh/docs/server-deployment)指南，在没有桌面环境的服务器上运行 Nowledge Mem。

---

## 安装步骤

1. **Install Mem** — Download and install the application
2. **First Boot** — Launch the Nowledge Mem application
3. **Get Models** — Download local models
4. **Add Extension** — Capture from AI conversations

---

### 步骤 1：安装应用

#### macOS

将 Nowledge Mem 拖到 `/Applications` 文件夹。

#### Windows

下载并运行安装程序。

#### Linux

下载并安装 AppImage 或对应发行版的安装包。

---

### 步骤 2：启动应用

首次启动时，双击"应用程序"文件夹中的 Nowledge Mem 图标。

#### 首次启动故障排除

如果应用启动时间过长或显示错误：

- **服务超时**：如果你看到"启动服务时间过长"，这通常意味着全局代理阻止了对 `localhost` 的访问。禁用代理后重试。
- **macOS 版本**：确保你运行的是 macOS 15 或更高版本。不支持旧版本。

需要更多帮助？查看[故障排除指南](https://mem.nowledge.co/zh/docs/troubleshooting)获取日志查看方式和详细诊断。你也可以把日志发到社区，或通过邮件联系支持。

---

### 步骤 3：安装所需模型

启动 Nowledge Mem 后，按照应用提示安装所需模型（总共约 2.4GB）：

#### 设备端 LLM 平台支持

| 平台 | 支持情况 |
|------|---------|
| Apple 芯片 Mac | 支持设备端 LLM |
| Windows | 需要远程 LLM |
| Intel Mac | 需要远程 LLM |
| Linux | 需要远程 LLM |

#### 1. 检查通知

你会在应用右上角看到下载提示。

#### 2. 导航到模型

点击通知按钮，或前往**设置 → 模型**。

#### 3. 安装模型

在应用提示下载的模型卡片上点击**安装**。下载将自动开始，你可以监控进度。

#### 下载时间

根据你的网络连接，下载可能需要 5-15 分钟。模型只需下载一次。

---

### 可选：安装浏览器扩展

如果你希望把受支持的 Web AI 聊天平台里的对话也接入 Mem，可以额外安装 **Nowledge Mem Exchange** 浏览器扩展。它会捕获值得保留的洞察，也能保存完整对话备份。

- [Chrome / Edge](https://chromewebstore.google.com/detail/nowledge-memory-exchange/kjgpkgodplgakbeanoifnlpkphemcbmh)

安装后，点击扩展图标打开侧边栏。在**设置**中配置你的 LLM 提供商以启用自动捕获。

#### 受支持的 Web AI 聊天平台

ChatGPT、Claude、Gemini、Microsoft Copilot、Perplexity、DeepSeek、Kimi、Qwen、POE、Manus、Grok 等。扩展可以从受支持的网站捕获高价值洞察，也可以保存完整对话备份。详情请参阅[浏览器扩展指南](https://mem.nowledge.co/zh/docs/integrations/browser-extension)。

---

## 安装之后做什么

安装应用本身，只是先把 Mem 装到你的机器上。下一步不是把所有能力一次配满，而是先完成一个最小可用流程：

1. 在 Timeline 里保存一条记忆
2. 为你已经在用的工具选择一条路径
3. 在继续配置其他东西之前，先确认它真的工作

对大多数用户来说，先看这三条主路径就够了：

- 如果你的工具已经有**原生集成**，优先安装原生集成：Claude Code、Gemini CLI、Droid、Cursor、OpenClaw、Alma
- 如果你的工作主要发生在 ChatGPT、Claude、Gemini、Microsoft Copilot 等网页聊天里，就安装**浏览器扩展**
- 如果你想先理解 Mem 本体，再连接外部工具，就先直接使用应用本身

其他路径，例如复用型工作流包、`nmem` CLI 和直接 MCP，只有在你的工具真的需要时再考虑。

如果你已经明确想采用"一台常开 Mem，多台设备接入"的方式，建议先看[多设备同步](https://mem.nowledge.co/zh/docs/sync)，再按照[随处访问](https://mem.nowledge.co/zh/docs/remote-access)完成配置。

---

## 移动端 App（预览版）

iOS 和 Android 原生客户端现已推出预览版。它们通过[随处访问](https://mem.nowledge.co/zh/docs/remote-access)连接你桌面上的 Mem——数据始终留在你的主机上，移动端提供随时随地的搜索和记录能力。

- **iOS**：[加入 TestFlight]
- **Android**：[下载 APK]

两个客户端都需要桌面端 Mem 保持运行并开启"随处访问"。

---

## 下一步

---


---

# 快速入门

> 来源：https://mem.nowledge.co/zh/docs/getting-started

你的前五分钟

---

连接工具之前先做这一件事：先在 Timeline 里保存一条真实记忆，再确认你能把它找回来。如果你还没选好路径，请先看[从这里开始](https://mem.nowledge.co/zh/docs/start-here)。

---

## Timeline

打开 Nowledge Mem，你看到的就是这个界面：

Timeline 是你的主界面。

---

## 先存下一条你想记住的内容

写下你刚刚做出的一个决定，或者一段对话里的一个洞察。按回车。

剩下的交给 Mem：标题、关键概念、图谱连接都会自动处理。你只需要先把内容写下来。打开图谱视图后，你会看到它已经和相关记忆连在一起了。

---

## 提一个问题

输入一个问题：

> "上个月我对认证方案做了什么决定？"

答案来自**你自己的知识**，不是互联网。每次提问时，Mem 都会搜索你的记忆，并基于相关内容组织答案。

---

## 放入一个 URL 或文件

粘贴一个 URL，页面内容自动解析并索引。拖入 PDF、Word 文档、演示文稿，同样处理。每次输入都在扩展你的知识库。

---

## 先拿到第一个明确结果

在配置任何集成之前，先在应用里确认最基础的一步已经工作：保存一条记忆，再把它问回来，确认答案确实基于你自己的知识。

如果你想要更具体的验证清单，请看[如何确认 Mem 已经在工作](https://mem.nowledge.co/zh/docs/verify-it-works)。

---

## 迁入已有对话

如果你不是从零开始——手头已有编程助手本地会话、ChatGPT / DeepSeek / ChatWise / Alma 等官方导出、或单条 `.md` 对话——请先看[导入已有对话](https://mem.nowledge.co/zh/docs/import-existing-conversations)。它说明对话 → 导入里各按钮对应：本机编程扫描、厂商批量包、单文件，以及扩展在当前网页会话上能做什么，不必在菜单里瞎试。

---

## 连接你的第一个工具

最合适的路径，取决于你实际在用什么工具。大多数人只需要下面其中一条：

> **一次只走一条路径**：不要一口气把所有集成入口都装上。先选一条和你当前主力工具最匹配的路径，确认有效后再扩展。

在选择之前，先记住一个关键区别：

| 路径 | 它真正给你的是什么 |
|------|-------------------|
| **原生集成** | 效果最强。只要宿主支持，就可以借助原生 hooks 或生命周期能力。 |
| **复用型工作流包** | 能把 Working Memory、检索、提炼这些行为教给模型，但仍然主要靠模型自己执行。 |
| **直接 MCP** | 只是把工具接进去；如果你不再补上系统提示或规则，通常不会自动变成"会主动记忆"的智能体。 |

所以，同样接到一个 Nowledge Mem 服务器上，不同宿主路径的实际体验会很不一样。

---

### 1. 如果你的工作主要发生在 ChatGPT、Claude 或 Gemini 网页版

安装[浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension)。

你的第一个成功状态可以很简单：

1. 打开侧边栏
2. 如果需要，先把它连接到 Mem
3. 捕获或提炼一段对话
4. 在 Mem 里看到对应的记忆或线程

---

### 2. 如果你的工具已经有专属的 Nowledge 集成

就先走这条路径：

- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)
- [Claude Desktop](https://mem.nowledge.co/zh/docs/integrations/claude-desktop)
- [Gemini CLI](https://mem.nowledge.co/zh/docs/integrations/gemini-cli)
- [Copilot CLI](https://mem.nowledge.co/zh/docs/integrations/copilot-cli)
- [Cursor](https://mem.nowledge.co/zh/docs/integrations/cursor)
- [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw)
- [Alma](https://mem.nowledge.co/zh/docs/integrations/alma)

这些集成已经把工作记忆简报、检索、提炼，以及适合该工具的保存方式准备好了。

---

### 3. 如果你的编程助手支持共享技能或提示词包

如果你的工具还没有专属集成，但支持通用工作流包，就安装对应的包：

```bash
npx skills add nowledge-co/community/nowledge-mem-npx-skills
```

这条路径特别适合 OpenCode 和很多其他智能体环境。它会给你的智能体加上一组可复用能力：搜索、读取工作记忆简报、提炼，以及 `save-handoff`。

如果你的工具有自己的复用型工作流包，就直接跟着那份指南走。例如 Codex 应该使用它自己的 [Codex CLI 指南](https://mem.nowledge.co/zh/docs/integrations/codex-cli)，而不是通用的 `npx skills` 包。

---

### 4. 如果你只是想先在本机拿到 CLI

如果你想先用最轻量的方式开始，打开**设置 > 偏好设置 > 开发者工具**，安装随应用附带的 CLI。

这样会把 `nmem` 命令装到你的机器上，适合手动命令、脚本，以及能调用本地终端命令的智能体环境。它**不等于**已经安装了原生集成，也**不等于**已经装好了完整的通用工作流包。

---

### 5. 如果你的客户端只支持 MCP

如果你的工具支持 MCP，但没有专属的 Nowledge 集成，就把下面这段 JSON 加到它的 MCP 设置里：

```json
{
  "mcpServers": {
    "nowledge-mem": {
      "url": "http://127.0.0.1:14242/mcp",
      "type": "streamableHttp"
    }
  }
}
```

如果你还是不确定该走哪条路径，就回到[集成](https://mem.nowledge.co/zh/docs/integrations)按工具查找。

---

## 其他内容进入 Mem 的方式

- **已有对话（先看地图）**：[导入已有对话](https://mem.nowledge.co/zh/docs/import-existing-conversations)——本机编程助手、厂商导出包、单文件、扩展与当前标签页的配合方式
- **网页里正在聊的这一帖**：[浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension)针对你在侧栏里配合使用的那段会话；整库历史请走应用的批量导出再批量导入
- **对话格式（参考）**：[对话（Threads）](https://mem.nowledge.co/zh/docs/threads)了解 Markdown 规则与批量文件细节
- **手动创建**：在记忆视图中点击 **+ 创建**，或在任何终端中使用 `nmem m add`（[CLI 参考](https://mem.nowledge.co/zh/docs/cli)）

---

## 试试这些

保存几条记忆后，在 Timeline 中试试：

- **"哪些想法变化最大？"** — 找到经历了多次修订的想法，按时间线讲述演变过程。
- **"总结我最近的编程对话"** — 如果你的编程对话已经通过自动同步、原生保存路径或导入进入 Mem，就会列出并总结最新的编程会话。
- **"在我的文档中搜索 [主题]"** — 全文搜索资料库中所有内容：PDF、电子表格、代码文件，任何你放进来的东西。

用得越久越强大：你的知识越多，这些查询就越有价值。持续使用一段时间后，结果会明显不一样。

---

## 下一步

- [从这里开始](https://mem.nowledge.co/zh/docs/start-here)：为你的真实工作流选择第一条路径
- [使用 Nowledge Mem](https://mem.nowledge.co/zh/docs/usage)：日常工作流、搜索，以及知识如何成长
- [如何确认 Mem 已经在工作](https://mem.nowledge.co/zh/docs/verify-it-works)：验证应用闭环和工具连接
- [记忆](https://mem.nowledge.co/zh/docs/memories)：可搜索、可连接、可演变的原子化知识
- [导入已有对话](https://mem.nowledge.co/zh/docs/import-existing-conversations)：一页看清所有导入路径
- [对话](https://mem.nowledge.co/zh/docs/threads)：格式、导入界面与提炼
- [AI Now](https://mem.nowledge.co/zh/docs/ai-now)：拥有你全部知识的个人 AI 助手
- [集成](https://mem.nowledge.co/zh/docs/integrations)：为每个 AI 工具选择合适的连接路径

---


---

# 导入已有对话

> 来源：https://mem.nowledge.co/zh/docs/import-existing-conversations

本机编程会话、应用导出包、浏览器当前页——Mem 目前支持这三条路。

---

可以把已有对话迁进 Mem 的方式主要是三种：**扫描本机编程助手会话**、**从应用里导出的文件**，以及**在浏览器里对当前正在看的会话用扩展捕获**。下面按你的情况对应到**对话 → 导入**（以及**集成 → 对话导入**）里的按钮。

格式细节：批量文件名、Markdown 标题和边角情况见[对话（Threads）](https://mem.nowledge.co/zh/docs/threads)。

---

## 先选对场景

| 你有什么 | 在 Mem 里点哪里 | 说明 |
|---------|----------------|------|
| 本机 Claude Code、Cursor、Codex CLI、OpenCode 会话 | 对话 → 导入 → **查找 AI 对话**（集成 → 对话导入里也有入口） | 扫描后由你勾选再导入 |
| ChatGPT / Claude（claude.ai 或 Claude Desktop）/ DeepSeek / ChatWise / Alma 的官方整包导出，或 Raycast AI 的导出 JSON | 对话 → 导入 → **批量导入** | 一个文件（见下表） |
| 一条 `.md` 等单文件 | 对话 → 导入 → **单个对话** | 见[对话里的「单个对话」](https://mem.nowledge.co/zh/docs/threads#%E6%96%87%E4%BB%B6%E5%AF%BC%E5%85%A5) |
| 浏览器里正在进行的那段网页对话 | [浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension) | 只针对你**当前与扩展配合使用的那一个会话**，不是整套账号历史 |

---

## 1. 编程助手（本机）

**查找 AI 对话**会在本机找 Claude Code、Cursor、Codex CLI、OpenCode 的会话数据。在你确认之前不会写入 Mem。

有原生插件时也可以长期同步；这里的扫描更适合**换机或第一次接进来**时补一段历史。

---

## 2. 导出文件（批量）

**批量导入**要用各应用**官方给你的**那份文件：

| 应用 | 文件 | 导出路径 |
|------|------|---------|
| **ChatGPT** | 数据包里的 `chat.html` | 设置 → 数据控制 → 导出数据 |
| **Claude** | 邮件里的 `data-…-batch-….zip`（含 `conversations.json` 与 `memories.json`） | 网页或 Desktop：头像 → 设置 → 隐私 → 导出数据，Anthropic 会发下载链接（[官方说明](https://support.claude.com/en/articles/9450526-how-can-i-export-my-claude-data)）。iOS/Android 上无法发起导出。 |
| **DeepSeek** | `deepseek_conversations.json` | chat.deepseek.com → 设置 → 数据 → 导出数据 |
| **ChatWise** | 全部聊天的 `.zip` | 在 ChatWise 导出 |
| **Alma** | 带 `threads.json` 的 `alma-backup-…zip` | 设置 → 数据 → 导出全部对话 |
| **Raycast AI** | 默认 `raycast_ai_chats.json`（或该工具生成的任意 `.json`） | Raycast 无官方整包导出 — 使用 [raycast-ai-exporter](https://github.com/daveonkels/raycast-ai-exporter)（macOS，见项目 README） |

用下载下来的原文件即可；Mem 会识别格式。

#### Claude 导出 ZIP

请使用邮件里的 ZIP（例如 `data-2026-04-01-08-10-35-batch-0000.zip`）。Mem 会从 `conversations.json` 读入全部对话，并在你勾选的对话导入完成后，把 `memories.json` 里 Claude 保存的**档案型记忆**写入资料库为一条带导入标注的记忆卡片。若只解压并选择单独的 `conversations.json`，则仅导入对话、不包含该记忆。

#### Raycast AI（macOS）

该工具为第三方脚本：需打开 Raycast 的 **AI Chat** 窗口，为本机终端授予**辅助功能**权限，脚本会写出结构化 JSON（多会话、`role` / `content` 消息）。文件里的日期为**近似值**（与侧栏分组有关）。同一 JSON 可走**批量导入**或 `nmem t import --file`。

---

## 3. 浏览器：只管当前焦点会话

扩展**不会**下载你在网页版里的全部聊天记录。它处理的是你**正在看、并与扩展一起用的这一个会话**。要厂商**一次性打包的全部历史**，用**批量导入**（或在本机能扫到编程助手会话时走扫描）。

---

## 4. 单文件

**单个对话**用于对话 Markdown、Cursor 导出等单文件——详见[对话](https://mem.nowledge.co/zh/docs/threads#%E6%96%87%E4%BB%B6%E5%AF%BC%E5%85%A5)。

---

## 在应用里怎么找

- **对话 → 导入**：查找 AI、批量、单个。
- **集成 → 对话导入**：同一套说明，并可跳到对话视图。

---

## 接下来

- [对话](https://mem.nowledge.co/zh/docs/threads) — 格式与提炼
- [入门](https://mem.nowledge.co/zh/docs/getting-started) — 第一次打开应用
- [集成](https://mem.nowledge.co/zh/docs/integrations) — 插件、MCP、扩展

---


---

# 如何确认 Mem 已经在工作

> 来源：https://mem.nowledge.co/zh/docs/verify-it-works

用最快的方式确认 Nowledge Mem 是否真的已经在应用和你的工具里正常工作。

---

当你能明确地指着一个结果，说出下面这些话时，Mem 才算真的开始工作：

- 这确实来自我自己的知识
- 这段对话确实被保存进来了
- 这个工具确实用上了我已经存下来的上下文

这一页的目标，就是帮你尽快证明这一点。

---

## 核心测试

在测试任何集成之前，先确认应用本身最基础的一步已经成立：

### 1. 保存一条记忆

在 Timeline 里写下一条真实的事实、决定或偏好，然后按回车。

### 2. 再把它问回来

针对这条记忆直接问一句：

> 我之前对部署做过什么决定？

### 3. 看回答是否真正基于你自己的内容

如果回答明显反映了你刚刚保存的内容，而不是泛泛而谈，说明 Mem 已经在工作。

---

## 如果你先只用应用本身

你应该能做到这三件事：

1. 在 Timeline 里保存一条记忆
2. 再次搜索并找到它
3. 问一个问题，并得到基于你自己知识的回答

如果这一步成立，说明应用本身已经能正常工作。

---

## 如果你使用浏览器扩展

当下面几项都成立时，说明 Mem 已经接通：

- [ ] 扩展可以正常打开侧边栏
- [ ] 如果你使用远程访问，连接测试能够成功
- [ ] 至少有一段网页对话被成功捕获或提炼进 Mem
- [ ] 你能在应用里把它打开成一条记忆或一个线程

如果你还没有达到这个状态，就回到[浏览器扩展指南](https://mem.nowledge.co/zh/docs/integrations/browser-extension)。

---

## 如果你使用编程助手

当工具能够调用你之前保存过的上下文时，才算是真的接通。

可以这样测试：

1. 先在 Mem 里保存一条简短决定，例如你偏好的缓存方案。
2. 通过这个工具的专属集成打开你的编程助手。
3. 提一个本应受这条上下文帮助的问题。

例如：

> "我之前对缓存方案做过什么决定？"
> "继续我之前在认证模块上的做法。"
> "搜索我以前关于 Redis 的决策。"

当工具不再让你从头解释，而是能调用你已有的知识时，这条连接才算真的建立。

请使用对应指南：

- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)
- Cursor
- Gemini CLI
- Copilot CLI
- [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw)
- [Alma](https://mem.nowledge.co/zh/docs/integrations/alma)
- [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli)

---

## 如果你使用 AI Now

当下面几项成立时，说明 AI Now 已经工作正常：

- [ ] 你已经为所需功能配置好了远程 LLM
- [ ] 你提问的是和自己已保存知识相关的内容
- [ ] 回答明显建立在你的记忆、文件或已启用插件之上，而不是泛泛的模型回答

如果 AI Now 无法启动，或无法利用你的已保存上下文，请查看 [AI Now](https://mem.nowledge.co/zh/docs/ai-now) 和[故障排除](https://mem.nowledge.co/zh/docs/troubleshooting)。

---

## 如果你在多台设备上使用 Mem

当下面这些都成立时，说明你的同步已经正常工作：

- [ ] 第二台设备能用同一个 Mem URL 和 API Key 成功连接
- [ ] 在一个客户端创建的记忆或线程，会出现在另一个客户端里
- [ ] 两边搜索到的是同一套知识库内容

如果你还不确定 Mem 这里说的"同步"到底是什么模型，先看[多设备同步](https://mem.nowledge.co/zh/docs/sync)。具体配置步骤请看[随处访问](https://mem.nowledge.co/zh/docs/remote-access)。

---

## 正常工作的信号

- 搜索结果里能看到你自己的措辞、决定或引用
- 捕获的对话出现在 Threads 里
- 你连接的工具能调用过去的决策，而不需要你重新粘贴
- AI Now 能够基于你的知识来回答问题

---

## 值得警惕的信号

- 你得到的始终是泛化模型输出，看不出和你保存内容的关系
- 你一次配置了很多路径，但没有任何一条给出明确结果
- 在还没确认应用本身已经工作之前，就开始测试 MCP 或 CLI
- 希望浏览器捕获、编程助手接入和 AI Now 同时证明自己

---

## 如果你仍然觉得不清楚

回到最小路径：

1. [从这里开始](https://mem.nowledge.co/zh/docs/start-here)
2. 在 Timeline 保存一条记忆
3. 先验证应用本身
4. 只连接一个你真实在用的工具

---

## 下一步

- [从这里开始](https://mem.nowledge.co/zh/docs/start-here)：先选对第一条路径
- [快速入门](https://mem.nowledge.co/zh/docs/getting-started)：了解 Timeline 工作流
- [集成](https://mem.nowledge.co/zh/docs/integrations)：用正确方式连接合适的工具
- [多设备同步](https://mem.nowledge.co/zh/docs/sync)：理解"一台 Mem，多端接入"的同步模型
- [故障排除](https://mem.nowledge.co/zh/docs/troubleshooting)：诊断启动、连接与会话问题

---



---

# 第二部分：核心功能

# 使用 Nowledge Mem

你的知识如何通过 Timeline、AI 工具、搜索和 AI Now 为你服务

如果你还不知道先怎么用 Mem，先回到[从这里开始](https://mem.nowledge.co/zh/docs/start-here)和[如何确认 Mem 已经在工作](https://mem.nowledge.co/zh/docs/verify-it-works)。当你已经先完成一次保存、找回、再给工具使用的过程以后，这一页会更容易理解。

## Timeline

Timeline 是你的主页。你捕获的、你提问的、系统自己发现的，都在一个流里。

### 你会看到什么

Timeline 中会出现这些类型：

| 项目 | 说明 |
|------|------|
| Capture | 你保存的记忆，自动生成标题和标签 |
| Question | 你的提问和 AI 基于知识库给出的回答 |
| URL Capture | 抓取、解析并存储的网页 |
| Insight | 系统在你的记忆之间发现的关联 |
| 知识结晶 | 对多条相关记忆的综合提炼 |
| 标记 | 矛盾信息、过时内容或需要验证的观点 |
| 工作记忆 | 你的每日晨间简报 |

不需要手动整理。

## 你的 AI 工具

把你真正使用的 AI 工具连接到知识库里。Claude Code、Cursor、Codex、OpenCode、Alma、DeepChat、LobeHub 等，都可以通过合适的路径连接回同一个记忆系统。

如果你还在判断某个工具该怎么接入，请先看[集成](https://mem.nowledge.co/zh/docs/integrations)。这一页讲的是：当其中一条路径已经接通之后，Mem 用起来会是什么感觉。

**没有 Mem：**

"帮我给 API 加缓存。"

智能体通常会继续追问你用什么技术栈、什么基础设施、什么偏好。你从头解释一遍。

**有 Mem：**

"帮我给 API 加缓存。"

智能体可以搜索你的知识，找到上个月的 Redis 决策和 API 限流方案，更快写出符合你架构的代码，少很多重复解释。

在集成做得好的情况下，这一步不需要你反复提醒。原生集成、配置好的复用型工作流包，或带有清晰意图规则的 MCP 客户端，都会在需要时主动去找你的知识。

今天在 Claude Code 中保存一条洞察，明天 Cursor 遇到同一主题时自动找到。不需要导出，不需要复制。

你也可以直接询问 Agent："上个月我对数据库迁移做了什么决定？"，它会搜索你的知识来回答。

详见集成了解设置说明。

## 搜索

### 应用内

按 `Cmd + K`（macOS）打开记忆搜索。搜索理解语义，不仅仅是关键词。搜索"设计模式"会找到关于"架构方法"的记忆。

三种搜索模式协同工作：

- **语义搜索**：按含义查找记忆
- **关键词搜索**：精确匹配特定术语
- **图搜索**：通过连接和关系发现记忆

### 全局搜索

全局启动器让你无需打开 Nowledge Mem 就能搜索。在任何应用中按 `Cmd + Shift + K`，搜索后直接将结果粘贴到需要的地方。如果你使用 [Raycast](https://raycast.com)，[Nowledge Mem 扩展](https://mem.nowledge.co/zh/docs/integrations/raycast) 可以将同样的搜索直接带入你的启动器。

## 知识如何随时间成长

持续使用几周后，系统会开始在后台帮你整理线索。

周二你保存了一条 PostgreSQL 决策。周四你提到 CockroachDB 是迁移目标。周五早上，Working Memory 简报写道："你的数据库选型在演变。"

这就是[后台智能](https://mem.nowledge.co/zh/docs/advanced-features)：

- **知识演变** — 检测到你对同一话题的想法在变化，自动链接各个版本。
- **知识结晶** — 将分散的记忆综合为参考文章。
- **标记** — 发现过去和现在的想法矛盾时主动提示。
- **工作记忆** — 每日简报，AI 工具开始会话时自动读取。

**使用条件**

后台智能需要已配置的远程 LLM，以及你的当前版本所对应的许可能力。

## AI Now

运行在你本地的个人 AI 工作区，可以使用你保存的知识、连接的笔记、文件和已启用的插件。详见 [AI Now](https://mem.nowledge.co/zh/docs/ai-now) 完整指南。

## 命令行

`nmem` CLI 让你从任何终端获得完整访问：

```bash
# 搜索你的记忆
nmem m search "authentication patterns"

# 添加记忆
nmem m add "We chose JWT with 24h expiry for the auth service"

# JSON 输出用于脚本
nmem --json m search "API design" | jq '.memories[0].content'
```

详见 [CLI 参考](https://mem.nowledge.co/zh/docs/cli) 获取完整命令集。

## 远程 LLM

默认在本地运行，不需要联网。知识库增长后，远程 LLM 能给你更强的处理能力。

**可用性**

远程 LLM 配置取决于你当前使用的版本和许可方式。

**解锁的功能：**

- [后台智能](https://mem.nowledge.co/zh/docs/advanced-features)：自动发现关联、生成知识结晶、产出洞察以及每日简报
- 更快的知识图谱提取
- 更细腻的语义理解
- AI Now Agent 能力

**隐私：**

你的数据仅发送到你选择的 LLM 提供商，永远不会发送到 Nowledge Mem 服务器。你可以随时切换回纯本地模式。

1. **打开远程 LLM 设置**
2. **开启远程模式**
3. **添加提供商与 API 密钥**
4. **测试连接、选择模型并保存**

测试连接，选择模型，保存。

## 下一步

- [从这里开始](https://mem.nowledge.co/zh/docs/start-here)：为你的真实工作流选择最简单的第一条路径
- [如何确认 Mem 已经在工作](https://mem.nowledge.co/zh/docs/verify-it-works)：确认搜索、捕获与工具连接
- [记忆](https://mem.nowledge.co/zh/docs/memories)：创建、搜索、组织和连接你的知识
- [对话](https://mem.nowledge.co/zh/docs/threads)：捕获、浏览和提炼 AI 对话
- [资料库](https://mem.nowledge.co/zh/docs/library)：导入文档，与记忆一起搜索
- [AI Now](https://mem.nowledge.co/zh/docs/ai-now)：基于你的知识进行深度研究和分析
- [后台智能](https://mem.nowledge.co/zh/docs/advanced-features)：知识图谱、洞察、知识结晶、工作记忆
- [你的档案](https://mem.nowledge.co/zh/docs/profile)：让 Mem 了解你是谁，代理的结果会更准确
- [集成](https://mem.nowledge.co/zh/docs/integrations)：为每个 AI 工具选择合适的连接路径

---

# 记忆

你的知识，一次一个想法。创建、搜索、组织和连接记忆。

记忆就是一条值得长期留下来的内容：一个决策、一条洞察、一个事实、一个流程、一次经验。每条记忆都应该能够独立成立，不依赖原始对话也能被理解。

记忆是 Nowledge Mem 最核心的单位。搜索、知识图谱、知识结晶、每日简报，以及你连接的 AI 工具之所以越来越有用，都是因为下面有这些记忆在支撑。

**记忆和对话怎么选？**

如果你要保留的是长期有价值的结论，请用记忆；如果你需要保留原始上下文和完整来龙去脉，请用对话。最常见的强路径是：先保存或导入对话，再把值得留下来的部分提炼成记忆。

## 第一条值得保存的记忆

如果你是新用户，先不要纠结结构。先保存一条真实内容，例如：

- 一个你已经做出的决定
- 一条你刚学到的经验
- 一个你反复使用的工作流程

然后再去 Timeline 里把它问回来。只要你能把它重新问出来，这一页后面的内容就会更容易理解。

## 记忆的结构

| 字段 | 说明 |
|------|------|
| 标题 | 简短摘要。通过时间线捕获时自动生成，也可手动设置 |
| 内容 | 知识本身，支持 Markdown |
| 标签 | 分类，用于过滤和组织 |
| 重要性 | 0.1 到 1.0 的评分，影响搜索排名和简报优先级 |
| 创建时间 | 时间戳，用于时间搜索和知识演化追踪 |

### 重要性等级

| 范围 | 含义 | 示例 |
|------|------|------|
| 0.8 – 1.0 | 关键 | 架构决策、突破性发现、生产事故 |
| 0.5 – 0.7 | 有用 | 常规决策、良好洞察、项目心得 |
| 0.1 – 0.4 | 背景 | 参考信息、细节备忘、随手记录 |

默认值为 0.5。AI 工具和后台智能使用此评分来排序搜索结果和每日简报。

### 标签

标准分类：

| 标签 | 用途 |
|------|------|
| insight | 关键学习、领悟、"原来如此"时刻 |
| decision | 选择及其理由和权衡 |
| fact | 重要数据点、参考信息 |
| procedure | 操作指南、工作流程、分步说明 |
| experience | 事件、对话、结果 |

新记忆在创建时会自动打标签。系统根据内容分配 2–4 个标签，并优先复用已有标签，保持分类体系一致。你随时可以编辑、添加或移除标签。

也支持自定义标签。标签使用小写加连字符格式（`api-design`、`team-standup`）。在记忆视图中按标签筛选，聚焦特定领域。

## 创建记忆

### 在时间线中

在顶部输入框中输入并按 Enter。AI 自动识别你的意图：

- 想法变成带自动标题和标签的**记忆**
- 问题从已有知识中获取**回答**
- URL 被**抓取**、解析并索引
- 文件被**解析**并存储

详见[快速上手](https://mem.nowledge.co/zh/docs/getting-started)。

### 从 AI 对话中

[浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension) 可从受支持的 Web AI 聊天平台捕获记忆：

- **自动捕获**：持续监控对话，自主保存有价值的内容
- **手动提炼**：让你对特定对话触发捕获

### 从对话中提炼

导入一段对话，然后提炼为独立记忆。每条提取的记忆都有自己的标题、标签和重要性评分。这就是把数小时的 AI 对话转化为可搜索、可连接知识的方式。详见[对话](https://mem.nowledge.co/zh/docs/threads)。

### 从 AI 工具中

已连接的 AI 工具可以在你工作时读取上下文、搜索旧知识，并保存值得长期保留的记忆。具体会怎么工作，取决于你选择的接入路径：

| 集成方式 | 如何保存记忆 | 配置 |
|----------|-------------|------|
| 原生集成（Claude Code、Gemini CLI、Droid、Cursor、Alma、OpenClaw） | 专属集成会教智能体何时检索、何时提炼、何时新增记忆，某些场景下也会更新已有记忆 | 集成 |
| 复用型工作流包（`npx skills`、OpenCode 与更多智能体） | 共享技能或工作流包代你调用 `nmem` | 快速上手 |
| 直接 MCP（没有专属包的 MCP 客户端） | 智能体直接调用 `memory_add` 与 `memory_update` | 集成 |

最好的集成不只是把 `memory_add` 暴露出来，还会教智能体先搜索，再判断是新增一条记忆，还是更新一条已经存在的记忆，避免越用越重复。

### 通过命令行

```bash
# 添加记忆（自动生成标题）
nmem m add "新服务选择 PostgreSQL，因为 jsonb 支持和团队熟悉度"

# 指定标题和标签
nmem m add "选择 PostgreSQL，因为 jsonb 支持和团队熟悉度" \
  --title "数据库决策：PostgreSQL" \
  --labels "decision,infrastructure" \
  --importance 0.8
```

### 通过 API

```bash
curl -X POST http://127.0.0.1:14242/memories \
  -H "Content-Type: application/json" \
  -d '{
    "content": "新服务选择 PostgreSQL",
    "title": "数据库决策",
    "labels": ["decision", "infrastructure"],
    "importance": 0.8
  }'
```

完整文档见 API 参考。

## 搜索记忆

### 在应用中

按 `Cmd + K`（macOS）或 `Ctrl + K`（Windows/Linux）打开搜索。三种搜索模式协同工作：

- **语义搜索**：按含义查找，搜索"设计模式"能找到关于"架构方案"的记忆
- **关键词搜索**：做精确匹配
- **图谱搜索**：通过实体连接和主题聚类发现记忆

在任意应用中按 `Cmd + Shift + K` 可直接搜索，无需打开 Nowledge Mem。

### 从 AI 工具中

已连接的 AI 工具会通过原生集成、共享工作流包或 MCP 工具来自动搜索。当当前话题很可能关联到过往工作时，做得好的接入路径会主动查询你的知识库，而不是等你额外提醒。

### 通过命令行

```bash
# 语义搜索
nmem m search "认证模式"

# JSON 输出用于脚本
nmem --json m search "API 设计" | jq '.memories[0].content'
```

## 编辑和组织

### 更新记忆

在记忆视图中点击任意记忆，编辑内容、标题、标签或重要性。更改立即生效。

通过命令行：

```bash
# 更新重要性
nmem m update <memory-id> --importance 0.9

# 更新标签
nmem m update <memory-id> --labels "decision,infrastructure,critical"
```

### 删除记忆

在记忆详情页面删除，或通过命令行：

```bash
nmem m delete <memory-id>
nmem m delete <memory-id> -f  # 脚本或非交互环境请加 -f
```

`nmem m delete` 默认会先询问确认。在脚本、Agent 或任何非交互终端里，请加 `-f`，否则命令会停在等待输入。

## 记忆如何连接

启用[后台智能](https://mem.nowledge.co/zh/docs/advanced-features)后，记忆会自动生长连接：

- **知识图谱** — 每条记忆成为图的一个节点。系统提取实体（人物、技术、概念）并映射关系。搜索"分布式系统"能找到关于"Node.js 微服务"的记忆，词不匹配，但含义匹配。
- **知识演化** — 保存一个已记录过的主题的新内容时，系统创建版本链接：**替换**、**丰富**、**确认**或**质疑**。追溯你对任何主题的理解如何随时间变化。
- **知识结晶** — 当足够多的记忆覆盖同一主题时，系统将它们合成为一篇参考文章。引用来源。后续保存相关内容时，知识结晶自动更新。
- **工作记忆简报** — 每天早晨，Mem 会基于你近期和重要记忆生成简报。Default 分区保留 `~/ai-now/memory.md`；如果你使用了 [Spaces](https://mem.nowledge.co/zh/docs/spaces)，其他分区也会有各自的简报。

详见[后台智能](https://mem.nowledge.co/zh/docs/advanced-features)完整指南。

## 记忆来源

| 来源 | 方式 | 了解更多 |
|------|------|----------|
| 时间线 | 输入后按 Enter | [快速上手](https://mem.nowledge.co/zh/docs/getting-started) |
| 浏览器扩展 | 自动捕获或手动提炼 Web AI 对话 | [浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension) |
| 对话提炼 | 从导入的 AI 对话中提取 | [对话](https://mem.nowledge.co/zh/docs/threads) |
| AI 工具 | Skills、插件或 MCP `memory_add` | [集成](https://mem.nowledge.co/zh/docs/integrations) |
| 命令行 | `nmem m add` | [CLI 参考](https://mem.nowledge.co/zh/docs/cli) |
| API | `POST /memories` | [API 参考](https://mem.nowledge.co/zh/docs/api) |

## MCP 工具

| 工具 | 功能 |
|------|------|
| memory_search | 按含义、关键词或图谱连接搜索记忆 |
| memory_add | 创建新记忆，包含内容、标题、标签和重要性 |
| memory_update | 更新内容、标题、重要性或标签 |
| memory_delete | 删除一条或多条记忆 |
| list_memory_labels | 列出所有标签及使用次数 |
| read_working_memory | 读取今日简报 |

## 下一步

- [对话](https://mem.nowledge.co/zh/docs/threads)：导入和提炼 AI 对话为记忆
- [资料库](https://mem.nowledge.co/zh/docs/library)：导入文档，与记忆一起搜索
- [AI Now](https://mem.nowledge.co/zh/docs/ai-now)：拥有你全部知识的个人 AI 助手
- [后台智能](https://mem.nowledge.co/zh/docs/advanced-features)：记忆如何自动生长连接
- [浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension)：从 Web AI 对话中捕获记忆
- [集成](https://mem.nowledge.co/zh/docs/integrations)：通过原生集成、复用包或 MCP 连接你的 AI 工具

---

# 对话

捕获、浏览、搜索和提炼来自任何 AI 工具的对话。

对话是你的会话层。它保留了 AI 会话原本发生的过程：你问了什么、工具回答了什么、任务是怎样一步步推进的。

它让你之后还能重新搜索过去的讨论、回到当时的上下文，并把其中真正值得长期保留的部分提炼成[记忆](https://mem.nowledge.co/zh/docs/memories)。

**需要完整来龙去脉时，用对话**

如果你需要保留原始消息，请保存或导入对话；如果你只需要长期有价值的结论，请提炼成记忆再继续使用。

**导入路径总览**

若你在选择如何把已有对话迁入 Mem（批量文件、编程助手扫描、浏览器还是单条 Markdown），请先读[导入已有对话](https://mem.nowledge.co/zh/docs/import-existing-conversations)。本文仍是「对话」功能的格式与细节说明。

## 第一条有用的对话

如果你是新用户，先做下面任意一件事：

- 导入一条你本来就在意的对话
- 让一个受支持的工具捕获一条完整会话
- 通过浏览器扩展捕获一段 Web 对话

然后打开这条对话，从里面提炼出一条真正有价值的记忆。这也是多数人第一次真正感受到对话为什么值得保存的时刻。

## 浏览对话

对话视图集中展示所有已导入的会话。

- **搜索** 对话内容或标题
- **筛选** 来源（Claude Code、ChatGPT、Cursor 等）
- **收藏** 重要对话，方便随时查看
- **查看** 对话中的每条消息

从侧边栏打开对话，或按 `Cmd + 3`（macOS）。

## 对话提炼

这里是对话和记忆真正连起来的地方。打开任意一条对话并触发提炼，系统会从会话中提取独立记忆。每条记忆都有自己的标题、标签和重要性评分，之后会进入知识图谱，并和其他内容一起参与搜索。

这也是把几小时的 AI 对话，慢慢变成可连接知识的方式。

普通大小的对话会直接开始提炼。很长的对话现在会提供**智能后台提炼**：几秒后由 Knowledge Agent 在后台启动，按需渐进式阅读这条对话，再保存一小组真正值得长期保留的记忆。它不会像过去那样在前台一次性硬读完整个长对话，所以更稳，也更适合大线程。

也可从浏览器中[浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension)支持直接从 Web 对话中智能提炼，无需先导入完整对话即可捕获记忆。

## 对话是如何进入 Mem 的

对话进入 Nowledge Mem 的方式不止一种。它们彼此相关，但并不相同：

- **原生插件与扩展**：像 Claude Code、Gemini CLI、Droid、Cursor、OpenClaw、Alma 这样的工具专属集成
- **本地自动同步**：应用内发现，会监控你机器上受支持的编程智能体对话
- **共享技能或工作流包**：比如 `npx skills`、Pi 之类的共享工作流
- **浏览器捕获**：Exchange 扩展只处理你在侧栏里实际配合使用的那一个网页会话，不是整账号历史；总览见[导入已有对话](https://mem.nowledge.co/zh/docs/import-existing-conversations)
- **手动导入**：文件、导出记录、API 调用和 CLI 导入

这里最重要的区分是：

- **完整会话捕获**：Mem 收到的是这个工具里真实录制的整段对话
- **交接摘要**：Mem 保存的是一个可继续工作的摘要，而不是整段会话本身

共享 skills 也应该放在这个框架里理解。它们可以在很多智能体里复用，但除非宿主运行时真的暴露了可读取的会话文件或稳定的 transcript API，否则它们不能诚实承诺"完整会话捕获"。

多数用户只需要记住一条判断：

- 如果你的工具已经支持真实会话保存，就直接用那条路径
- 如果它今天只支持交接摘要，就保持这个心智模型清楚；需要完整历史时，请使用导入或自动同步

### 自动同步

#### 应用内发现

扫描本机编程工具的对话并自动导入，无需手动导出。

**仅适用于本机发现**

这条路径会扫描运行 Nowledge Mem 的那台机器上的对话文件，很适合做本地同步。但它和 `nmem t save --from ...` 不是一回事。后者会在客户端本机读取会话文件，再把规范化后的会话线程上传到远程 Mem。

**第一次导入之后会发生什么**

第一次导入会先把这段对话存成线程。打开自动同步后，Mem 会继续为同一线程追加新消息；如果某个应用还能稳定暴露项目路径，Mem 也会记住这个识别出的项目，把后续会话继续带进来。

| 客户端 | 同步方式 | 入口 |
|--------|---------|------|
| Claude Code | 自动发现 + 增量同步 | 对话 → 导入 → 查找 AI 对话 |
| Cursor | 自动发现 + 增量同步 | 对话 → 导入 → 查找 AI 对话 |
| Codex | 自动发现 + 增量同步 | 对话 → 导入 → 查找 AI 对话 |
| OpenCode | 自动发现 + 增量同步 | 对话 → 导入 → 查找 AI 对话 |

### 原生集成里的保存路径

不同集成提供的对话保存能力并不相同。有些支持完整会话捕获，有些会在生命周期事件里自动捕获，而 Droid 和 Cursor 当前仍然把插件内的交接摘要与完整对话导入分开。

| 集成 | 保存的是什么 | 工作方式 | 配置 |
|------|-------------|---------|------|
| Claude Code | 完整会话捕获 | 退出时通过 Stop 钩子自动保存当前会话，也支持显式 `/save`。`nmem` 会先在客户端本机读取会话文件，再上传。 | Claude Code 指南 |
| Gemini CLI | 完整会话捕获 + 单独的交接摘要 | `save-thread` 通过 `nmem t save --from gemini-cli` 导入 Gemini 记录下来的会话；`save-handoff` 继续单独保存可恢复的交接摘要。 | Gemini CLI 指南 |
| Droid | 插件内的交接摘要 | Droid 插件刻意只暴露 `save-handoff`，不暴露 `save-thread`。它已经能提供工作记忆简报、路由式检索与可恢复交接，但真实 transcript 级会话导入会留到未来真正具备运行时支持时再提供。 | Droid 指南 |
| Cursor | 插件内的交接摘要 | Cursor 插件刻意只暴露 `save-handoff`，不暴露 `save-thread`。如果你要导入真实 Cursor 对话，请使用应用内发现。等未来有真正的实时会话导入器后，再提供对应的会话线程保存能力。 | Cursor 指南 |
| Alma | 完整会话捕获 | 空闲 2 分钟、切换线程、退出应用时自动保存对话（默认开启）。 | Alma 指南 |
| OpenClaw | 完整会话捕获 | 每次智能体运行结束时自动捕获会话，并支持可选的 LLM 提炼。 | OpenClaw 指南 |
| Hermes Agent | 真实会话边界上的完整会话捕获 | 原生记忆提供者会在正常退出、`/new`、`/reset` 和网关会话正常过期时捕获清洗后的 `user`/`assistant` 对话记录。 | Hermes 指南 |
| Codex CLI | 完整会话捕获 | 通过显式 `/save` 导入 Codex 记录下来的会话。 | Codex CLI 指南 |
| Copilot CLI | 完整会话捕获 | Stop hook 会在每次回复后把新记录下来的 Copilot 对话内容追加进 Mem；如果用户明确要求保存，还可以另外写入一条简洁的摘要线程。 | Copilot CLI 指南 |
| 通用 `npx skills` 智能体 | 仅交接摘要 | 使用 `save-handoff`。共享 skills 可以指导保存行为，但无法在所有宿主上承诺 transcript 级导入。 | 集成总览 |

**完整会话捕获 与 交接摘要**

如果你需要的是准确的历史对话，请优先使用完整会话捕获或导入路径。交接摘要适合中断后继续，不等于保存整段会话本身。

### 文件导入

#### 批量导入

一次性导入导出文件中的全部对话。

| 来源 | 文件格式 | 如何导出 |
|------|----------|---------|
| ChatGPT | chat.html | ChatGPT 设置 → 数据控制 → 导出数据 |
| Claude | data-…-batch-….zip（含 conversations.json、memories.json） | claude.ai 或 Claude Desktop：头像 → 设置 → 隐私 → 导出数据（Anthropic 说明）；移动端不支持 |
| DeepSeek | deepseek_conversations.json | chat.deepseek.com → 设置 → 数据 → 导出数据 |
| ChatWise | .zip（含 JSON 文件） | 从 ChatWise 导出全部聊天 |
| Alma | alma-backup-YYYY-MM-DD.zip（内含 threads.json） | Alma 设置 → 数据 → 导出全部对话 |
| Raycast AI | .json（如 raycast_ai_chats.json） | 无官方导出 — macOS 上使用 raycast-ai-exporter（见其 README） |

#### 单个对话

从文件导入一条对话。

| 格式 | 文件类型 | 备注 |
|------|----------|------|
| 对话 Markdown | .md | `## User` / `## Assistant` / `## System` 标题，可选 YAML 前置信息 |
| Cursor | .md | Cursor 原生导出格式（自动识别） |
| 通用 Markdown | .md | 任意 Markdown 文件，作为文档导入 |

**文档还是对话？**

如果你的 `.md` 文件是普通文档（没有 `## User` / `## Assistant` 标题），它属于[资料库](https://mem.nowledge.co/zh/docs/library)，而非对话。拖入时间线或从资料库视图导入。

### 对话 Markdown 格式

通用的对话导入格式。任何输出 `## User` / `## Assistant` 标题的工具，生成的文件都能被 Nowledge Mem 识别。

#### 最简示例

最简单的格式，两轮对话，无前置信息：

```markdown
## User
Python 的 GIL 是什么？

## Assistant
全局解释器锁（GIL）是 CPython 中的互斥锁，同一时刻只允许一个线程执行 Python 字节码。这意味着 CPU 密集型的多线程程序不会因多线程而加速——可以改用多进程或异步 I/O。
```

#### 完整示例

包含可选的 YAML 前置信息和系统消息：

```markdown
---
title: Python 异步编程
source: chatgpt
date: 2025-06-15
---

## System
你是一位资深 Python 开发者，擅长清晰地解释技术概念。

## User
Python 的 async/await 是怎么工作的？

## Assistant
Python 的 `async`/`await` 让你编写在等待 I/O 时不阻塞的并发代码。
`async def` 函数返回一个协程，`await` 会暂停它直到结果就绪——同时其他协程可以继续执行。

## User
什么时候该用 asyncio，什么时候该用线程？

## Assistant
**asyncio** 适用于 I/O 密集型任务（HTTP 请求、数据库查询、文件读写）——比线程更轻量，扩展性更好。
**threading** 适用于调用不支持异步的阻塞库。
**multiprocessing** 适用于 CPU 密集型任务。
```

#### 格式规则

- **标题**：`## User`、`## Assistant` 或 `## System`，二级标题，每条消息一个
- **内容**：标题之间的所有内容为一条消息，Markdown 格式、代码块和列表会原样保留
- **前置信息**：文件开头可选的 YAML 块，支持 `title`、`source`、`date` 字段，均为可选
- **识别**：包含至少一个 `## User` 或 `## Assistant` 标题的文件会被自动识别为对话
- **兜底**：不含上述标题的文件作为单条文档消息导入
- **大小写**：角色名不区分大小写（`## user` 和 `## User` 均可）

## Import API

完整的请求/响应结构和字段说明

## CLI

`nmem` 命令行支持从文件、JSON 或标准输入导入对话。

```bash
# 导入对话 Markdown 文件
nmem t import --file conversation.md

# 指定标题和来源
nmem t import --file chat.md --title "Python 异步" --source chatgpt

# 从 JSON 消息导入
nmem t import --messages '[{"role":"user","content":"你好"},{"role":"assistant","content":"嗨"}]'

# Raycast AI 多会话 JSON（raycast-ai-exporter 输出）
nmem t import --file ~/Desktop/raycast_ai_chats.json

# 从标准输入导入 Markdown
cat conversation.md | nmem t import --stdin --title "管道导入"
```

运行 `nmem t import --help` 查看所有选项。完整命令列表见 [CLI 参考](https://mem.nowledge.co/zh/docs/cli)。

## 浏览器扩展

Nowledge Mem Exchange 在受支持的 AI 聊天网页上工作，针对的是你**正在与扩展一起使用的这一帖会话**（例如侧栏对准的标签页）。它**不会**把网页版聊天记录整库搬下来；要全部历史，请用各平台官方导出，再在 Mem 里批量导入，或先看[导入已有对话](https://mem.nowledge.co/zh/docs/import-existing-conversations)。自动捕获、手动提炼、备份当前会话等能力见[浏览器扩展指南](https://mem.nowledge.co/zh/docs/integrations/browser-extension)。

## MCP 工具

| 工具 | 功能 |
|------|------|
| thread_persist | 将编程会话保存为对话线程 |
| thread_search | 按关键词搜索对话或列出最近的对话 |
| thread_fetch_messages | 获取特定对话的完整消息 |
| search_thread_messages | 在指定对话中按关键词搜索消息 |

## 下一步

- [记忆](https://mem.nowledge.co/zh/docs/memories)：提炼后的知识：创建、搜索和组织
- [资料库](https://mem.nowledge.co/zh/docs/library)：导入文档，与记忆一起搜索
- [浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension)：从 Web AI 平台捕获对话
- [集成](https://mem.nowledge.co/zh/docs/integrations)：通过原生集成、复用包或 MCP 连接你的 AI 工具
- [API 参考](https://mem.nowledge.co/zh/docs/api)：完整的 REST API 文档

---

# 资料库

导入文档，与记忆一起搜索

资料库是让文件变成可用知识的地方。

它适合那些应该整体保留下来的来源材料：PDF、报告、电子表格、幻灯片、Markdown 笔记和代码。Mem 会解析、索引，并让这些内容和你的记忆一起工作，而不是变成孤立的附件。

**资料库和记忆怎么分工？**

资料库存放你想整体保留的来源材料，记忆保存的是长期有价值的结论。一个很自然的顺序是：先导入文档，先问问题；只有当你希望其中的知识长期进入图谱和记忆系统时，再执行精读。

把一份 40 页的架构评审拖进资料库。在 Timeline 里问："评审文档里关于 API 限流是怎么说的？"回答会同时引用文档第 12 页的内容，以及你三个月前保存的 Redis 限流决策。文档和记忆会一起参与搜索。

资料库可以存放 PDF、电子表格、Word 文件、演示文稿、代码和其他格式。系统会解析内容、切分片段并建立索引。文档进入**可搜索**状态后，就会出现在你日常思考时真正用到的地方：

- **AI Now**：按文件名、主题或一个具体问题提问。AI Now 会检索资料库、读取匹配段落，并和你的记忆一起给出带引用的回答。
- **Timeline Feed Agent**：后台把对话转化为记忆的内置 Agent，现在也可以在线程提到某份文档时反向进入资料库内容。
- **Graph Intelligence Agent**：在探索知识图谱时，Source 节点现在可以被直接检索和读取；你可以从一组相关记忆直接深入到对应的原始文档，而不用离开画布。
- **通过 MCP 连接的 AI 工具**：Claude Code、Cursor 以及任何支持 MCP 的客户端都可以调用 `query_sources`、`read_source_content`、`search_source_chunks` 和 `analyze_source_data`，让它们的回答真正建立在你自己的文件之上。
- **`nmem` 命令行**：终端与脚本工作流可以用 `nmem sources search`、`read`、`search-chunks` 和 `analyze` 检索、阅读并分析资料库。详见 CLI 参考。

## 第一份有用的文档

如果你是新用户，先导入一份你真的在意的文档。然后在 Timeline 里针对它问一个具体问题。

最基本的一步就是：

1. 先加进来一份真实来源
2. 再提一个有上下文的问题
3. 看答案怎样同时使用文档内容和你已有的知识

第一次验证到这里就够了。精读可以等到这个基础闭环已经有价值时再做。

## 支持的格式

| 格式 | 扩展名 | 处理方式 |
|------|--------|---------|
| PDF | .pdf | 感知版面提取文本，分段，索引 |
| Word | .docx | 解析为文本并提取图片，分段，索引 |
| 演示文稿 | .pptx | 提取幻灯片内容和图片，索引 |
| 电子表格 | .xlsx, .csv | 解析为 Markdown 表格，索引。多工作表 XLSX 按标签页展示 |
| Markdown | .md | 直接解析并索引 |
| 纯文本 | .txt, .org | 直接索引 |
| 代码 | .py, .js, .ts, .rs, .go, .java, .c, .cpp, .rb, .swift | 索引 |
| URL | .html, .pdf | 转换为 Markdown，索引 |

## 添加文档

将文件拖入 Timeline 输入框，或在资料库视图中导入。也可以直接拖入文件夹——其中所有支持的文件都会自动导入。

文档经过以下处理流程：

1. **解析**：从文件格式中提取内容
2. **分段**：切分为可搜索的段落
3. **索引**：加入向量索引和关键词索引

处理状态可在资料库视图中查看。索引完成后，文档进入**可搜索**状态，可以立刻在对话、全局搜索和已连接的 AI 工具里使用。

## 可搜索 vs. 精读

资料库中的每份文档处于以下两种状态之一：

| 状态 | 含义 | 如何触发 |
|------|------|---------|
| 可搜索 | 内容已解析、分段、索引。在 Timeline 中提问时，AI 可以直接读取和引用该文档。 | 自动——导入文件时即发生。 |
| 已精读 | AI 完整分析文档，生成结构化记忆、图谱关联和交叉引用。文档中的知识永久加入你的记忆图谱。 | 手动——点击来源上的**精读**按钮。 |

在 Timeline 中直接询问某个文件时，AI 会读取这份文档当前可搜索的内容。精读则会进一步提取可长期复用的知识，并把它连接进你的记忆图谱。

## 精读会产生什么

当你对一个来源执行精读时，AI 会完整分析其内容。对电子表格，它会计算和理解结构化数据；对文档，它会通读文本并抽取重点。最终会生成：

- **知识结晶** — 当 3+ 条记忆围绕同一主题聚类时，生成可持续更新的知识结晶
- **矛盾检测** — 标记与现有知识的冲突（例如，新策略推翻了之前的决定）

精读需要一定的 AI 处理时间。完成后，结果数量会显示在流程指示器中，例如 `已精读 (5)`。

## 搜索文档

文档和记忆一起被搜索。在 Timeline 中问 "Q4 报告对用户流失怎么说的？"，搜索同时覆盖你的记忆和导入的文档。

在资料库视图中，可以按状态筛选——**可搜索**、**已精读**、**已过期**或**错误**——快速找到尚未精读或需要关注的来源。

## 与文档对话

在 Timeline 中直接对任何文档提问。答案同时来自文档和你的记忆，引用具体页码。

"架构评审文档里关于 API 限流是怎么说的？"会返回一个答案，其中既引用文档第 12 页，也引用你三个月前的 Redis 决策。

## 批量操作

在资料库视图中选择多份文档，可以：

- **发送到 AI Now** 进行跨文档分析——比较报告、综合发现，或提出跨文档的问题
- **批量精读** — 选择尚未精读的来源，点击 `精读 (N)` 批量处理。对于已经精读过的来源，按钮会显示 `Re-analyze (N)`，用于刷新提取出的知识

## 文档、记忆和对话

三种内容类型，各有不同的用途：

| 类型 | 是什么 | 示例 |
|------|--------|------|
| 记忆 | 原子化的洞察、决策或事实 | "新服务选择 PostgreSQL，因为 jsonb 支持" |
| 文档 | 整体导入的参考材料 | 一份 40 页的架构评审 PDF |
| 对话 | AI 会话存档 | 你在 ChatGPT 上关于异步模式的讨论 |

文档和对话是来源。记忆是提炼后的知识。当你精读文档或对话时，独立的洞察被提取为记忆并连接到知识图谱。原件保留在资料库或对话视图中作为来源。

## 下一步

- [快速入门](https://mem.nowledge.co/zh/docs/getting-started): Timeline 和所有添加知识的方式
- [后台智能](https://mem.nowledge.co/zh/docs/advanced-features): 导入的知识如何连接到你的图谱
- [搜索与相关性](https://mem.nowledge.co/zh/docs/search-relevance): 搜索如何在记忆和文档间排序结果
- [对话](https://mem.nowledge.co/zh/docs/threads): 捕获、浏览、搜索和提炼 AI 对话

---

# Spaces

用可选的分区，把工作、生活、项目或不同 Agent 的记忆分开。

Spaces 是一种可选的"分区"机制。它的作用不是把 Mem 拆成几套系统，而是在你真的需要时，让不同上下文各自更专注。

大多数人一开始不需要它。先一直用 `Default`，等你明确感觉到两个上下文已经互相打扰，再打开也不晚。

**先保持简单**

如果你还没有明显感觉到项目、团队或 Agent 之间的记忆混在一起，那就先别开 spaces，继续用默认分区。

## 第一个有用动作

只有在你已经知道"为什么要分开"时，再创建第二个 space。

最适合的第一批场景：

- 一个长期工作项目，不想和个人内容混在一起
- 一个会持续积累经验的 Agent
- 一个给团队共用的上下文分区

进入**设置 → 偏好设置 → Memory spaces**，打开它，然后创建一个名字明确的分区。

**成功应该是什么样**

在新 space 里保存一条记忆，切回 `Default`，确认它不会出现在默认的 Memories 视图里。再切回去，能立刻找到它。

## Spaces 会改变什么

当某个 space 处于激活状态时，下面这些界面默认都跟着它走：

- Memories
- Threads
- Library
- Working Memory

AI Now 也会跟随当前 space，但你仍然可以切回看全部活动。

**实体图谱仍然是全局的**。Mem 还是会理解你整个知识网络。Spaces 改变的是日常默认读写的记忆类界面，而不是把图谱硬切开。

## 这些设置到底是什么意思

一个 space 里最关键的是这三个设置：

**这个空间里的自动回忆**

- **只留在这个空间**：开始回答前，第一步自动回忆只看当前分区。
- **留在这里 + 共享空间**：先看这里，再一起读你标记为共享上下文的空间。
- **搜索全部空间**：只给少数天生就该跨全局工作的分区用。

**作为共享上下文一起读的空间**

这里只影响检索范围。**不会**把记忆自动搬过去。**不会**把两个空间偷偷合并。

**智能体在这里该怎么工作**

AI Now、Feed 和内置后台任务在这个空间里工作时会读它。它影响的是检索和表达方式。**不会**改变记录保存在哪里。

## Spaces 里的 Working Memory

Working Memory 已经不再只是一个全局文件。

- `Default` 仍然保留兼容路径 `~/ai-now/memory.md`
- 其他 space 也会有各自的 Working Memory 简报
- 当连接的工具知道自己当前在哪个分区时，它会自动读取对应那一份

这样研究 Agent 打开时看到的是研究上下文，你的默认个人分区仍然可以保持安静。

如果某个 space 已经没有记忆、对话或资料库条目了，但还留着自己的 Working Memory 笔记，删除它时 Mem 会直接说明这一点，并允许你在同一步里把这些自动生成的内容一起移除。

## 移动已有记录

如果你是在使用 Mem 一段时间后才创建新的 space，不需要重新导入历史数据。

在 Memories 或 Threads 里进入选择模式，选中要移动的项目，然后用 Move 把它们移动到另一个 space。需要整理更多内容时，可以先选中当前页，再选择 All pages，一次移动或删除当前 space 里的全部主记录。

Shared context 不会移动记录。它只是在检索设置允许时，让一个 space 读取另一个 space 的内容。

## AI 工具怎么使用 Spaces

好的集成会把它当作"环境上下文"，而不是每一轮都让你重新解释一遍的新概念。

- **AI Now**：跟随应用里当前选中的 space
- **原生集成**：如果宿主已经知道当前项目或 Agent 分区，应该优先把这个 lane 存在自己的配置里
- **CLI**：需要时可以显式加上 `--space "<space name>"`
- **HTTP 和 MCP API**：仍然沿用 `space_id` 这个字段名做兼容，但也可以直接传可见的空间名称。

```bash
nmem --json wm read --space "Research Agent"
nmem --json m search "token rotation" --space "Research Agent"
nmem spaces
```

如果某个工具本身并没有自然的项目分区或 Agent 分区，就应该继续留在 `Default`，不要硬造一个。

**优先用宿主自己的配置**

如果一个集成本身已经有设置页或 provider 配置，就应该优先在那里选择 lane。环境变量只适合没有更好配置面的 CLI-first 工具。

## 多 Agent 宿主怎么设计

不是所有集成都能以同样的粒度处理 spaces。

- **编程类工具**，例如 Claude Code、Codex、Gemini CLI，通常只需要给整段会话设一个可选 space。
- **Agent 宿主**，例如 Hermes、Alma、Bub、OpenClaw，可能需要按 profile、进程或稳定的 Agent 身份来分配不同 lane。

更合理的做法通常只有三种：

1. **固定 lane**：一个 profile 或一个进程始终只用一个 space
2. **派生 lane**：宿主本来就知道稳定的身份、项目或工作区，再据此推导出 space
3. **明确映射**：宿主暴露一小组可枚举身份，再把它们一一映射到命名好的 spaces

**不要伪造路由**

如果宿主并没有可靠的身份或工作区信号，就不要强行做"每个 Agent 一个 space"的自动路由。此时最稳妥的选择仍然是：一个 profile 对应一个固定 lane，或者继续使用 `Default`。

## 什么情况下最值得用

Spaces 真正有价值的时候，是你想提高专注度，而不是为了"隔离而隔离"。

它尤其适合：

- 一个所有者，同时运行很多长期 Agent
- 工作和个人上下文明显不同
- 一个项目共用分区，再加一个更安静的默认分区
- 团队想共享上下文，但不想把每个人的私人笔记全混进去

## 现在先别担心什么

- 不是因为有这个功能，你就必须立刻建 work / life 两个 space
- 不需要为了 spaces 重做标签体系
- 不需要把实体图谱切开，Mem 本来就故意把它保持为全局

## 下一步

- [记忆](https://mem.nowledge.co/zh/docs/memories)：如果你想先看捕获和搜索怎么配合
- [AI Now](https://mem.nowledge.co/zh/docs/ai-now)：如果你想让 Agent 工作区跟随当前分区
- [后台智能](https://mem.nowledge.co/zh/docs/advanced-features)：如果你想理解 Working Memory 和后台任务在 spaces 下怎么运作
- [集成](https://mem.nowledge.co/zh/docs/integrations)：如果你想让已连接的 AI 工具自然跟随当前分区

---

# AI Now

基于你的知识库的个人 AI 智能体工作区

AI Now 是你直接使用已保存知识的工作区。它运行在本机上，可以把你的记忆、附加文件和已启用插件一起当作真实任务的上下文。

它和普通聊天窗口的区别在于，它不是从空白开始。AI Now 会从你已经积累下来的知识出发，所以它更适合做研究、分析、演示文稿，以及那些应该建立在你自己历史之上的多步骤任务。

**要求**

AI Now 需要配置[远程 LLM](https://mem.nowledge.co/zh/docs/usage#远程-llm)。

前往**设置 → 远程 LLM** 进行设置，详情参考[远程 LLM](https://mem.nowledge.co/zh/docs/usage#远程-llm)。

**客户端模式**：如果你已通过[随处访问](https://mem.nowledge.co/zh/docs/remote-access)连接到另一台 Nowledge Mem，AI Now 会自动使用远程服务器的 LLM 配置，无需在本地设置。

## 第一个值得做的任务

如果你是第一次打开 AI Now，先从一个明确依赖你自己知识的问题开始：

> 我做过哪些关于缓存的架构决定？

这比泛泛地闲聊更适合作为第一次体验，因为它会直接把产品模型展示出来：

AI Now 会读取你已经保存的知识，在相关时结合文件或已启用插件，必要时还可以把结果继续沉淀回知识库。

**第一次成功应该是什么样**

只要 AI Now 能对一个明确依赖你自己知识的问题给出有用回答，而且你不需要从头把背景重新讲一遍，这条路径就已经成立了。

## AI Now 能做什么

| 类别 | 功能 |
|------|------|
| 记忆搜索 | 通过语义理解找到相关记忆 |
| [资料库搜索](https://mem.nowledge.co/zh/docs/library) | 搜索、阅读并分析你导入资料库的文档：PDF、电子表格、Word、幻灯片、代码 |
| 深度研究 | 结合你的记忆和网络搜索的多源研究 |
| 文件分析 | 分析你提供的 Excel、CSV、Word、PDF 文件 |
| 数据可视化 | 根据你的数据生成图表 |
| 演示文稿 | 创建幻灯片，带实时预览和 PowerPoint 导出 |
| 旅行规划 | 创建交互式逐日行程 |
| 集成 | 连接 Notion、Obsidian、Apple Notes 和自定义 MCP 插件 |

## 快速入门

### 1. 配置远程 LLM

前往**设置 → 远程 LLM**并添加你的 API 密钥。

### 2. 打开 AI Now

点击侧边栏中的**AI Now**标签，或按 `Cmd/Ctrl + 5`。

### 3. 开始一个有根据的任务

先从一个和你已经给过 Mem 的上下文相匹配的任务开始：

> 我做过哪些关于缓存的架构决定？

在相关时，它会使用你的记忆、网络搜索，以及你已经连接并启用的笔记或插件能力来组织答案。

你也可以拖入文件或文件夹进行分析，或让它基于你的知识生成报告。在工作过程中，AI Now 也可以在合适的时候创建或更新记忆。

如果你使用了 [Spaces](https://mem.nowledge.co/zh/docs/spaces)，AI Now 会跟随应用里当前激活的 space。这样一个 AI Now 会话就可以专注在某个项目或某个 Agent 分区里，而不需要改动整套 Mem 的使用方式。

## 在聊天中引用记忆

使用 `@` 搜索并在对话中引用特定记忆。

## 深度研究

AI Now 可以运行并行子任务，跨多个来源搜索并综合结果。

**启用深度研究**

在 AI Now 聊天界面中点击**研究**切换以启用深度研究。

**工作原理**

提出研究问题：

> 研究量子纠错的当前状态

AI Now 将：

1. 搜索你的记忆了解已有知识
2. 从多个角度搜索网络
3. 综合为一个答案
4. 引用来源并附带可靠性指标

## 可选技能

技能是针对特定任务的专门能力。

| 技能 | 启用的功能 |
|------|-----------|
| 文档 | Excel/CSV 分析、图表生成、文件操作 |
| 演示文稿创建器 | 幻灯片生成，带实时预览和导出 |
| 旅行规划器 | 交互式行程创建 |
| Browse Now | 用真实浏览器完成登录态和交互型网页任务 |

在 **AI Now → 插件 → 技能** 中启用技能。

## 文件分析

将文件或文件夹附加到你的对话中进行即时分析。

**文档技能**

在 AI Now 插件中切换**文档**技能以启用数据分析能力。

### 支持的文件

| 类型 | 扩展名 | AI Now 做什么 |
|------|--------|--------------|
| 电子表格 | .xlsx、.xls、.csv | 分析数据、发现模式、生成图表 |
| 文档 | .docx、.doc、.pdf | 总结、提取要点、回答问题 |
| 代码 | .py、.js、.ts 等 | 审查、解释、建议改进 |

### 示例

点击文件夹图标附加 `sales_q4.xlsx`

问："这个数据中的前 3 个趋势是什么？"

AI Now 分析并生成可视化

你也可以附加整个文件夹一次分析多个文件。拖入文件夹即可分析。

## 演示文稿

AI Now 可以创建带实时预览和编辑的演示文稿。

**演示文稿技能**

在 AI Now 插件中切换**演示文稿**技能以启用演示文稿创建能力。

### 创建幻灯片

> 根据我们上面的研究创建一个演示文稿，包括一些图表或图形来支持洞察

AI Now 生成结构清晰、包含图表和洞察的幻灯片。

### 编辑

生成后，通过后续请求进行优化：

- "让第三张幻灯片更有视觉效果"
- "添加一张关于客户细分的幻灯片"
- "简化结论"

或者，点击**编辑**按钮编辑演示文稿。

### 导出

点击 **PPTX** 按钮下载为 PowerPoint（.pptx）以在其他工具中使用。

## 旅行规划

AI Now 可以创建详细的旅行行程。

**旅行规划器技能**

在 AI Now 插件中切换**旅行规划器**技能以启用旅行规划能力。

> 规划一个以美食和文化为重点的 5 天东京之旅

AI Now 生成一个交互式逐日行程，包含活动、地点和提示，以你最近的记忆和深度研究作为上下文。

## Browse Now

**Browse Now 技能**

在 **AI Now → 插件 → 技能** 中打开 **Browse Now**，让 AI Now 在需要时调用你的真实浏览器。

当任务依赖的是你的浏览器会话，而不是普通抓取时，就该打开它。常见场景包括：

- 需要登录态的网站
- 需要点击、输入、滚动或多步操作的页面
- 需要截图或读取渲染后页面内容的任务
- 动态页面很多，普通抓取拿不到真实界面的情况

这项能力只能在本机使用。浏览器桥接运行在 AI Now 所在机器上，不会通过「随处访问 Mem」暴露出去。

## 插件

通过插件连接你的其他应用。

### 内置插件

#### Obsidian

连接你的本地 Obsidian 知识库：

1. 前往 **AI Now → 插件**
2. 启用 **Obsidian**
3. 设置你的知识库路径

现在 AI Now 可以与你的记忆一起搜索和阅读你的 Obsidian 笔记。

#### Notion

连接你的 Notion 工作区：

1. 前往 **AI Now → 插件**
2. 启用 **Notion**
3. 点击**连接**并在浏览器中授权

AI Now 现在可以搜索你的 Notion 页面和数据库。

#### Apple Notes (macOS)

在 macOS 上，AI Now 可以搜索和阅读你的 Apple Notes：

1. 前往 **AI Now → 插件**
2. 启用 **Apple Notes**
3. 在提示时授予权限

无需设置路径或同步——直接读取系统数据库，只读访问。

### 自定义 MCP 插件

AI Now 支持模型上下文协议（MCP）用于自定义集成。

1. **打开自定义插件**：前往 **AI Now → 插件 → 自定义插件**
2. **添加 MCP 服务器**：点击**添加 MCP 服务器**
3. **配置服务器**：配置服务器（stdio 命令或 HTTP 端点）
4. **测试连接**：点击**测试连接**进行验证
5. **启用插件**

## 会话管理

**自动批准模式**

为了更快的工作流程，启用**自动**以跳过文件操作和其他操作的确认提示。

**谨慎使用**

自动批准授予 AI Now 在不询问的情况下采取行动的权限。仅在可信工作流程中启用。

## 从其他设备使用 AI Now

如果你的 Nowledge Mem 运行在一台常开设备上（Mac Mini、服务器或办公桌面电脑），你可以从其他任何设备通过[随处访问](https://mem.nowledge.co/zh/docs/remote-access)使用 AI Now。

这不是另一套独立知识库，而是你在另一台客户端上继续使用同一个 Mem。想先理解整体模型，可阅读[多设备同步](https://mem.nowledge.co/zh/docs/sync)。

**工作原理**：

1. 在第二台设备上打开桌面应用，通过**设置 → 随处访问**连接到你的主 Mem
2. AI Now 在你正在使用的设备上本地运行
3. 它会自动获取远程服务器上配置的 LLM 提供商——无需额外设置
4. 你的记忆、对话和文库通过安全隧道从远程服务器访问
5. 标题栏显示**本地 AI** 表示 AI 智能体在本机运行，而数据在远程服务器上

**浏览器呢？**

AI Now 在浏览器的 `/app` 中不可用，因为 AI 智能体需要在本地运行。在浏览器中可以搜索、浏览记忆和探索知识图谱——如需使用 AI Now，请打开桌面应用。

**客户端模式下的插件**

Obsidian 和 Apple Notes 等插件从本机读取数据。远程使用 AI Now 时，这些插件访问的是你当前设备上的文件，而非远程服务器上的。

## 提示

- **要具体**："我们上个月关于数据库迁移做了什么决定？"比"数据库相关的东西"效果更好
- **添加上下文**：拖放文件或使用 `@` 引用特定记忆
- **使用会话**：不同项目用不同会话

## 下一步

- [随处访问](https://mem.nowledge.co/zh/docs/remote-access): 从任何设备使用 Mem——多设备工作的基础
- [多设备同步](https://mem.nowledge.co/zh/docs/sync): 理解"一台 Mem，多端接入"的同步模型
- [远程 LLM 设置](https://mem.nowledge.co/zh/docs/usage#%E8%BF%9C%E7%A8%8B-llm): 配置你的 AI 提供商
- [集成](https://mem.nowledge.co/zh/docs/integrations): 连接你的 AI 工具与捕获入口
- [后台智能](https://mem.nowledge.co/zh/docs/advanced-features): 你的知识如何自动成长
- [Spaces](https://mem.nowledge.co/zh/docs/spaces): 可选的分区机制

---

# 后台智能

你的知识如何自动成长：连接、洞察、知识结晶和每日简报

后台智能让 Mem 不只是把内容存下来。

你保存了记忆、对话和文档之后，系统还会继续在后台工作：把相关想法连接起来、找出矛盾、综合出知识结晶，并写出一份你的工具可以读取的每日简报。

一月，你保存了一个使用 PostgreSQL 的决策。七月，你又记录了正在迁移到 CockroachDB。你没有专门回头整理过这件事，但 Mem 会把两者连起来，追踪这段变化。下次你搜索其中任意一个主题时，都能看到这条思路是怎样一路演变过来的。

这些整理发生在后台。你下次打开应用时，相关连接就已经在那里了。

**先证明一个信号就够了**

不要一开始就试图验证后台智能的所有能力。更好的第一步是：先积累一些真实内容，然后确认 Mem 至少出现了一个真正有帮助的结果，比如一条相关的晨间简报、一条你自己没主动想到的连接，或一个被指出来的矛盾。

**使用条件**

后台智能需要已配置的远程 LLM，以及你的当前版本所对应的许可能力。在**设置 > 知识处理**中启用。

## 第一个有用的信号

当 Mem 开始出现下面这些结果时，就说明后台智能已经开始真正帮上忙了：

- 它指出了你自己未必会注意到的矛盾
- 它把跨时间的相关工作聚到一起
- 它写出的晨间简报确实和你今天要做的事相关

## 知识图谱

你保存的每条记忆都成为一个活的图谱中的节点。系统提取人物、技术、概念和项目，并将它们与你已有的知识关联。

结果是：搜索"分布式系统"就能找到你关于"Node.js 微服务"的记忆。用词不同，含义相通。

**自动 vs. 手动**

启用后台智能后，知识图谱提取会为新记忆自动运行。你也可以为旧记忆手动触发。

### 提取内容

当一条记忆被处理时，LLM 会识别：

- **实体**：人物、技术、概念、组织、项目
- **关系**：实体之间如何相互关联
- **与现有知识的连接**：与图谱中已有记忆的关联

你可以为任何记忆触发提取，方法是点击记忆卡上的 **Knowledge Graph** 按钮。

## 知识演变

当你保存了一个之前写过的主题的新内容，系统检测到关系并创建版本链接：

| 链接类型 | 发生了什么 | 示例 |
|----------|-----------|------|
| 替换 | 你改变了想法 | "使用 CockroachDB"替换"使用 PostgreSQL" |
| 丰富 | 你学到了更多 | "React 19 新增编译器"丰富了"React 18 并发渲染" |
| 确认 | 独立的认同 | 两篇独立评测推荐了同一个库 |
| 挑战 | 检测到矛盾 | 你三月份的评估与十月份的结论不一致 |

你可以追踪对任何主题的理解如何随时间变化。看到你在哪里改变了想法。理解原因。

## 社区检测

图算法会发现你知识里的自然聚类，也就是一组组彼此紧密相关的记忆。你的图谱里可能会慢慢浮现出"React 模式""API 设计""数据库优化"这样的主题区块，不需要你自己手动画出来。

在**图视图**中，点击**计算**运行社区检测。

## 可视化探索

你的知识，呈现为交互式网络。点击一条记忆，查看与它连接的一切。放大集群。追踪你从未想过要比较的主题之间的连接。

时间线滑块按日期范围过滤。观察某个领域的知识在数周或数月内如何增长。

## 系统会发现什么

图谱是基础。在此之上，后台智能主动分析你的知识，并将发现呈现在 Timeline 中。

### 洞察

最有用的洞察，往往是你自己原本不会主动去找的连接。

- **跨领域关联** — 三月你记录了 JWT refresh token 在支付服务中引发竞态条件。九月你在新认证服务中选了同样的 token 轮换方案。系统发现了：同一个失败模式，不同项目。
- **时间模式** — "你在两个月内第三次重新审视这个数据库迁移决策。"也许是时候做决定了。
- **被遗忘的上下文** — "你三月份的评估与十月份选择的方案相矛盾。"系统记住你写过什么，即使你自己忘了。

每条洞察都引用其来源。你可以自己追溯推理过程。

**质量优于数量**

一个改变你思维方式的连接，胜过十个显而易见的陈述。严格的质量门控把噪音挡在外面。

### 知识结晶

三个月内保存的五条关于 React 模式的记忆。散落在你的时间线中。难以拼凑。

知识结晶将它们综合为一篇参考文章。标注来源。新信息到达时自动更新。

你不需要专门去请求知识结晶。当系统手里已经有足够素材，可以整理出一篇真正有用的参考内容时，它就会自己出现。

### 标记

有时系统发现的是问题，而非连接：

| 标记类型 | 含义 | 示例 |
|----------|------|------|
| 矛盾 | 两条记忆存在分歧 | "使用 JWT token" vs "Session cookie 更安全" |
| 过时 | 更新的知识取代了旧的 | 一份 6 个月前的部署指南，已被最近的笔记覆盖 |
| 待验证 | 强烈的论断，无佐证 | 一条没有支持证据的单独断言 |

每个标记出现在 Timeline 中。你可以忽略、确认或链接到解决方案。

## 工作记忆简报

每天早上，Mem 会为当前分区写出一份 Working Memory 简报：

- 基于近期活动的活跃话题
- 需要你关注的未解决标记
- 知识库的近期变化
- 基于频率和近期度的优先事项

已连接的 AI 工具可以通过各自的接入路径在会话开始时加载这份简报。MCP 只是其中一种路径，原生集成和其他打包好的连接方式也可以做到。

`Default` 仍然保留兼容文件 `~/ai-now/memory.md`。如果你打开了 [Spaces](https://mem.nowledge.co/zh/docs/spaces)，其他分区也会通过同样的 Mem 接口拥有各自的 Working Memory 简报。

你仍然可以直接编辑 Working Memory。你的改动会被保留。

**与你的 AI 工具协同**

你的 AI 工具可以通过 MCP、原生集成或其他打包好的接入路径加载 Working Memory。只要工具知道当前处于哪个 space，就会自动读取对应那一份简报。

## 配置

在**设置 > 知识处理**中控制后台处理：

| 设置 | 默认值 | 控制内容 |
|------|--------|---------|
| 后台智能 | 关 | 所有后台处理的主开关 |
| 每日简报 | 开（启用时） | 每日工作记忆简报生成 |
| 简报时间 | 8 | 每日简报运行的时间（本地时间） |
| 自动提取 | 开（启用时） | 新记忆的自动知识图谱丰富 |

在 Linux 服务器上，通过 CLI 配置：

```bash
nmem config settings set backgroundIntelligence true
nmem config settings set autoDailyBriefing true
nmem config settings set briefingHour 8
```

## 下一步

- [记忆](https://mem.nowledge.co/zh/docs/memories)：创建、搜索、组织和连接你的知识
- [对话](https://mem.nowledge.co/zh/docs/threads)：捕获、浏览和提炼 AI 对话
- [快速入门](https://mem.nowledge.co/zh/docs/getting-started): Timeline、文档导入和所有添加知识的方式
- [集成](https://mem.nowledge.co/zh/docs/integrations): 通过原生集成、复用包、MCP 和浏览器捕获连接 AI 工具
- [故障排除](https://mem.nowledge.co/zh/docs/troubleshooting): 常见问题的解决方案

---

# 你的档案

让 Mem 了解你是谁，使代理给出更准确的回答、更贴切的标签和更有用的简报

你的档案决定了 Mem 的代理怎样理解你。不填的话，代理只能泛泛地处理。写上几句话，每次简报、标签和洞察就会更贴近你真正在做的事。

## 档案的作用

你填好档案之后，系统里的每个代理都会读取它：

- **每日简报**会围绕你的角色和当前工作来取舍重点
- **自动标签**会选择符合你思维习惯的分类，而不是泛泛的标签
- **知识提取**能理解你常用的领域术语
- **AI Now**每次对话一开始就已经知道你的背景

你只需要填一次。每个代理每次运行时都会读取。

## 设置你的档案

### 1. 打开档案

进入**设置 > 档案**

### 2. 姓名和别名

姓名帮助代理在导入的对话和会话中认出你。别名是你在其他平台上使用的标识（GitHub 用户名、Twitter 账号、Slack 显示名）。代理在处理来自不同来源的会话时，通过这些名称判断哪些消息是你说的。

### 3. 关于你

用几句话描述你的角色、当前的工作和兴趣。这是影响最大的字段。代理在处理你的知识时，靠它来判断什么重要。

示例：

- 金融科技创业公司的产品设计师，专注移动支付和新用户引导流程
- 正在为合规团队搭建知识库，减少重复的法务审查
- 对 AI 辅助写作、第二大脑方法论和个人知识管理感兴趣

不需要全写。哪怕只有一句话，也能改变代理排列优先级的方式。

### 4. 自定义指令

你可以在这里告诉代理，哪些事要和默认行为不一样。

示例：

- **标签**："客户反馈相关的记忆始终加上 'customer-voice' 标签。用项目代号，不要用全名。"
- **语言**："简报用法语写。编程概念保留英文技术术语。"
- **风格**："简报精简，不超过 5 条要点。跳过显而易见的关联。"

自定义指令会应用到每日简报、后台知识处理和 AI Now 对话中。

### 5. 输出语言

选择代理撰写简报、洞察、标签等生成内容时使用的语言。这和应用界面语言是分开的。

## 档案如何传递给代理

你的档案会注入到三条代理路径中：

| 代理 | 读取内容 | 用途 |
|------|---------|------|
| 后台智能 | 完整档案 + 自定义指令 | 围绕你的工作来组织简报、标签和洞察 |
| 知识代理 | 完整档案 + 自定义指令 | 指导 EVOLVES 检测、标签分配和知识提取 |
| AI Now | 完整档案 + 自定义指令 | 每次对话开始时就已加载你的上下文 |

连接的工具（Claude Code、Cursor 等）不会直接读取你的档案。它们间接受益：当搜索结果、标签和简报更准确时，工具拿到的上下文质量也更高。

## 建议

- **具体胜过全面。** "正在主导从 MongoDB 到 PostgreSQL 的迁移"比"十年经验的高级工程师"有用得多。
- **工作变了就更新。** 档案不是简历。换了项目，就更新一下。
- **自定义指令会积累效果。** 一条"始终用项目代号做标签"的规则，能省掉你手动重标几百条记忆的工作。
- **可以留空。** 没填的字段，代理会退回到默认行为。

## 下一步

- [后台智能](https://mem.nowledge.co/zh/docs/advanced-features)：看看代理怎样利用你的档案
- [AI Now](https://mem.nowledge.co/zh/docs/ai-now)：你的个人 AI 工作台，启动时就已加载你的上下文
- [集成](https://mem.nowledge.co/zh/docs/integrations)：连接那些能从更好上下文中受益的工具




---

# 第三部分：工具集成（总览与主要工具）

# 集成

Nowledge Mem 连接的是你真正每天在用的工具。知识留在一个地方，工具可以随时更换。

## 如果你是新用户

如果你刚装好 Mem，还不确定自己该走哪条路径，请先看[从这里开始](https://mem.nowledge.co/zh/docs/start-here)。等你知道自己需要原生集成、浏览器扩展，还是自定义路径以后，再回到这一页。

## 先理解"自主能力阶梯"

接入的是同一个 Mem 服务器，不同宿主给你的实际体验却可能差很多。

| 路径 | Working Memory | 检索与提炼 | 对话线程 |
|------|---------------|-----------|---------|
| **原生集成** | 通常能在会话开始时自动加载，或接进宿主的生命周期 | 能力最强；有的靠 hooks，有的靠引导，也有的是两者混合 | 有些宿主还能做到真实自动捕获或真实 transcript 保存 |
| **复用型工作流包** | 通常是"有引导" | 主要靠 rules、skills、prompts 去教模型主动用 | 大多只能做 handoff，或明确请求时再保存 |
| **直接 MCP** | 只有在你补上推荐提示后，才会更主动 | 本质上仍是"有引导"；光有 MCP tools 不会自动变成自治记忆 | 不要默认它具备真实 transcript 保存能力，除非宿主明确支持 |

最实用的规则只有一句：**宿主能装专属 Nowledge 路径时，就先装它。只有在没有更好入口时，才退回到通用 MCP。**

不过有一个值得单独说清的例外：有些宿主最好的体验其实是 hybrid。现在最典型的就是 Codex。先装 Codex 插件包，拿到 Working Memory、真实线程保存和 `nmem` 兜底；再把 Nowledge Mem MCP 服务器加进去，让检索和写入记忆变得更主动。

## 先选路径，再开始

Nowledge Mem 的接入方式有很多，但第一判断其实很简单：**如果你的工具已经有专属 Nowledge 集成，就先装它。只有在没有专属路径时，才考虑共享包、直接 MCP、CLI 或导入流程。**

对大多数用户来说，可以按这个顺序判断：

1. 你的工具如果已经有专属 Nowledge 集成，就安装它。
2. 如果你的工作主要发生在 ChatGPT、Claude、Gemini、Microsoft Copilot 等网页聊天里，就安装[浏览器扩展](https://mem.nowledge.co/zh/docs/integrations/browser-extension)。
3. 如果没有专属集成，但支持共享技能或提示词，就用复用型工作流包。
4. 只有当客户端支持 MCP、但没有更好的专属路径时，再直接配置 MCP。
5. 如果你需要手动命令、脚本或本地自动化，就直接使用 CLI。

| 如果你使用... | 推荐路径 | 集成类型 | 为什么 |
|-------------|---------|---------|-------|
| Gemini CLI | Gemini CLI 扩展 | 原生集成 | 专属扩展，提供生命周期钩子、命令、技能，以及基于 `nmem` 的真实会话导入 |
| Pi | Pi 指南 | 原生集成 | 五个可组合技能，基于 `nmem` 实现 Working Memory、路由检索和知识蒸馏 |
| Hermes Agent | Hermes 指南 | 原生集成 | 原生记忆提供者，提供 Working Memory 启动、每轮前检索，以及干净的 Nowledge 工具名 |
| 你已有的笔记 (Obsidian、Notion、Apple Notes) | 本地知识来源 | 在 AI Now 中把笔记与记忆一起搜索 |
| 历史会话或导出文件 | [对话指南](https://mem.nowledge.co/zh/docs/threads) | 导入入口 | 把文件、导出记录和过去的会话导入 Mem |

**大多数用户只需要看表里的一行**——找到你已经在用的那个工具，点进去照着那份指南做，先不用把这一整页都读完。

## 适合很多编程智能体的最快复用方案

对于支持 skills 安装器的智能体环境：

```bash
npx skills add nowledge-co/community/nowledge-mem-npx-skills
```

这会安装四个技能：`search-memory`、`read-working-memory`、`save-handoff` 和 `distill-memory`。安装后，智能体会在会话开始时先读取上下文，在需要时在记忆与对话之间选择合适的检索路径，并在用户明确要求时保存可恢复的交接摘要。

如果你的工具有自己专属的复用型工作流包，而不是通用的 skills 路径，就直接使用那份指南。比如 Codex 就应该跟着 [Codex CLI 指南](https://mem.nowledge.co/zh/docs/integrations/codex-cli) 来配置。

**有原生集成时，优先用原生集成**

如果你的工具已经有专属集成，请优先使用：[Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)、[Gemini CLI](https://mem.nowledge.co/zh/docs/integrations/gemini-cli)、[Droid](https://mem.nowledge.co/zh/docs/integrations/droid)、[Cursor](https://mem.nowledge.co/zh/docs/integrations/cursor)、[OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw)、[Alma](https://mem.nowledge.co/zh/docs/integrations/alma)、[OpenCode](https://mem.nowledge.co/zh/docs/integrations/opencode)、[Pi](https://mem.nowledge.co/zh/docs/integrations/pi)、[Hermes](https://mem.nowledge.co/zh/docs/integrations/hermes)。这些路径在共享记忆模型之上，还会加入工具专属的行为能力。

## 专属 Nowledge 集成 与完整会话捕获

如果你需要的是**真实录制的会话内容**，而不是一个可恢复的摘要，那么你走的是哪条集成路径就非常重要。

| 集成 | 是否有专属 Nowledge 包 | 是否支持完整会话捕获 | 说明 |
|-----|---------------------|-------------------|------|
| Claude Code | 是 | 是 | 原生插件，带生命周期钩子与真实会话导入。 |
| Gemini CLI | 是 | 是 | 原生扩展，提供 `save-thread`，并把 `save-handoff` 保持为单独语义。 |
| Droid | 是 | 目前插件内还不支持 | 原生插件只暴露 `save-handoff`。在 Droid 还没有真实 transcript 导入器之前，它不会假装提供 `save-thread`。 |
| Cursor | 是 | 目前插件内还不支持 | 原生插件只暴露 `save-handoff`。本地 Cursor 对话请使用应用内发现/导入。 |
| OpenClaw | 是 | 是 | 原生插件会自动捕获真实会话。 |
| Alma | 是 | 是 | 原生插件支持真实会话捕获。 |
| Codex CLI | 是 | 是 | 原生插件通过 `nmem t save --from codex` 导入真实会话记录。 |
| Copilot CLI | 是 | 是 | 原生插件通过 Stop hook 增量捕获 Copilot 已记录的会话内容。 |
| OpenCode | 是 | 是（本地自动同步） | 桌面应用轮询 OpenCode 的会话数据库自动导入。插件额外提供主动保存与交接工具。远程模式仅支持插件保存。 |
| Pi | 是 | 暂不支持 | 技能包通过 `save-thread` 提供结构化交接摘要，Pi 目前没有原生会话导入器。 |
| Hermes Agent | 是 | 支持，但发生在真实会话边界 | 原生记忆提供者负责 Working Memory、检索、保存指引，以及会话结束时的 transcript 线程捕获。 |
| 通用 `npx skills` 智能体 | 没有专属运行时导入器 | 否 | 应使用 `save-handoff`，而不是 `save-thread`。共享技能无法在各种宿主上稳定承诺真实 transcript 导入。 |

### 为什么通用 skills 不能承诺完整会话捕获

共享 skills 可以影响提示行为，但它并不能决定宿主智能体是否暴露可读取的会话文件，或稳定的 transcript API。正因为如此，通用 `npx skills` 才应该把 `save-handoff` 作为默认值。只有专属集成在对应运行时里真正具备能力时，才应该暴露真实的 `save-thread`。

如果你想看更完整的线程保存与导入矩阵，请继续看[对话指南](https://mem.nowledge.co/zh/docs/threads)。

## 面向自定义智能体的意图控制

### 1. 先给智能体一个记忆入口

在 `CLAUDE.md`、`AGENTS.md` 或 `AGENTS.md`（按宿主不同）里加入：

```markdown
## Nowledge Mem 集成

你可以使用 Nowledge Mem 进行知识管理。请主动使用这些工具：

- **读取工作记忆**：会话开始时调用 `read_working_memory` 获取简报
- **搜索记忆**：遇到与过去工作相关的问题时，调用 `memory_search` 或 `nmem m search`
- **保存洞察**：解决复杂问题或做出重要决策后，调用 `memory_add` 或 `nmem m add`
- **更新已有记忆**：在保存前先搜索，如果已有相关内容则更新而非重复创建
```

### 2. 加入一段简短、直接的意图策略

```markdown
### 记忆策略

在读取工作记忆后：
- 如果任务明显是在续接、复盘、查回归、准备发布，或追问过去的决策，不要停在简报这里，继续做一次有针对性的检索
- 不要在每一轮都重复读取，除非用户明确要求，或当前会话上下文已经明显变化。
- 如果宿主已经知道当前项目、Agent 或工作区所在的分区，就把 `--space "<space name>"` 一起带上。

在这些情况下主动搜索：
- 用户提到之前做过的工作、过去修过的问题，或更早的某个决策
- 当前任务是在继续某个已命名的功能、Bug、重构或子系统
- 当前的调试模式很像以前解决过的问题
- 用户在问理由、偏好、流程，或团队反复使用的工作方式

检索路由：
- 先用 `nmem --json m search` 查持久知识。
- 当用户问的是某次过去的讨论，或需要准确的对话历史时，再用 `nmem --json t search`。
- 如果结果里带有 `source_thread`，就用 `nmem --json t show <thread_id> --limit 8 --offset 0 --content-limit 1200` 分页查看，不要一口气加载整条长对话。

在保存知识时：
- 只有真正新的长期知识才用 `nmem --json m add`。
- 如果已有记忆已经记录了同一个决策、偏好或流程，而这次只是补充或修正，请改用 `nmem m update <id> ...`，不要制造重复。
- 交接摘要相关保存只在用户明确要求可恢复的交接摘要时使用。
```

如果你的智能体只能看到 MCP 工具，也可以沿用同样的策略，只是把命令名替换成工具名：`read_working_memory`、`memory_search`、`thread_search`、`thread_fetch_messages`、`memory_add`、`memory_update`。

### 3. 保持策略短、直接、可执行

最有效的意图提示，不是长篇说明，而是直接告诉智能体：
- 何时读取工作记忆简报
- 何时主动搜索
- 何时使用对话工具，而不是 `memory_search`
- 何时新增记忆，何时更新已有记忆
- 何时交接摘要只能在用户明确要求时执行

## 模型上下文协议 (MCP)

MCP 是 Nowledge Mem 面向通用客户端的兼容层。只有当你的客户端支持 MCP、但没有专属的 Nowledge 集成时，才应该优先走这条路径。

### 专属集成、复用包 与 直接 MCP

| 路径 | 适用场景 | 示例 |
|-----|---------|------|
| **原生集成** | 你的工具已经有专属的 Nowledge 包 | Claude Code 插件、Gemini CLI 扩展、Droid 插件、Cursor 插件、OpenClaw 插件、Alma 插件、OpenCode 插件、Pi 技能包、Hermes 记忆提供者 |
| **复用型工作流包** | 你的智能体可以安装共享技能或提示词 | `npx skills`、Pi 等共享工作流包 |
| **直接 MCP** | 客户端支持 MCP，你需要标准工具访问 | Cursor 手动 MCP 配置、Claude Desktop、ChatWise、GitHub Copilot |

### 如何理解 MCP

当有专属集成时，优先使用专属集成。它们底层可能会用到 `nmem`、MCP、工具原生生命周期钩子，或者几者混合，但对终端用户来说，首先应该按"我装的是哪种入口"来理解，而不是先去理解底层传输方式。对 Codex 这类少数宿主来说，MCP 不是替代插件的另一条路，而是建议和插件一起用的 companion。

### MCP 能力

- 搜索记忆：`memory_search`
- 读取工作记忆简报：`read_working_memory`
- 添加记忆：`memory_add`
- 更新记忆：`memory_update`
- 列出记忆标签：`list_memory_labels`
- 保存/导入对话：（内容延续）

### MCP Supported Services

- Cursor
- ChatWise
- Claude Desktop
- Claude Code
- Github Copilot
- Trae
- Codex
- Gemini Cli
- Qwen Code
- + any agent that supports MCP

### MCP 服务器配置

（配置细节请参考各客户端的具体文档）

### 自主行为的系统提示

对于只支持 MCP 的应用，把下面这段策略加入系统提示、`CLAUDE.md` 或 `AGENTS.md`，就能让智能体更主动、更稳定地使用记忆能力：

```markdown
## Nowledge Mem 集成

你可以使用 Nowledge Mem 进行知识管理，请主动使用这些工具：

**会话开始时（`read_working_memory`）：**
- 调用 `read_working_memory` 获取今日简报
- 了解用户当前的关注领域、优先事项和未解决问题
- 在与当前任务相关时自然引用这些上下文
- 如果任务明显是在续接、复盘、查回归、准备发布，或追问过去的决策，不要停在简报这里，继续做一次有针对性的检索

**何时搜索（`memory_search`）：**
- 当前话题与之前的工作有关联
- 当前问题与过去解决过的问题类似
- 用户询问过去的决策（"我们为什么选择 X？"）
- 复杂调试，可能匹配过去的根本原因
- 复盘、发布、文档对齐、集成行为异常这类问题，通常也值得在前面就先查一次

**何时搜索对话（`thread_search` / `thread_fetch_messages`）：**
- 用户在问某次之前的讨论或具体对话历史
- 某条记忆结果指向来源对话
- 按页逐步抓取消息，不要一次性倾倒整条长对话
- 只有在确实需要之前那段讨论本身时，再升级到对话检索

**何时保存记忆（`memory_add`）：**
- 解决复杂问题或完成调试后
- 做出重要决策并附带理由时
- 发现关键洞察（"原来如此"时刻）后
- 整理流程或工作方式时
- 在一段实质性工作结束前，主动检查一次：是否应该新增或更新一条真正值得留下的记忆
- 跳过：常规修复、进行中的工作、普通问答

**何时更新已有记忆（`memory_update`）：**
- 在保存前先搜索，看这个主题是否已经存在
- 如果检索结果已经包含同一个决策、偏好或流程，就更新原有记忆，而不是再新增一条近似重复的内容
- 当新信息是在补充、修正或延伸已有知识时，优先使用更新
```

这能让仅支持 MCP 的应用进入"有引导"的主动记忆模式。但它仍然不等于 hook 驱动的原生集成。所以，**只要你的宿主已经有专属 Nowledge 包，优先走专属路径。**

## 浏览器扩展

从受支持的 Web AI 聊天平台捕获记忆，支持自动捕获、手动提炼和对话备份。

[浏览器扩展指南](https://mem.nowledge.co/zh/docs/integrations/browser-extension)

## 对话

导入和管理来自编程工具、导出文件、API 或命令行的对话。

[对话指南](https://mem.nowledge.co/zh/docs/threads)

## 工具指南

按你实际使用的产品来找对应的安装与配置指南。这里既包含原生集成，也包含内置路径和可复用工作流包。

| 集成 | 你会得到 |
|-----|---------|
| Claude Code | 插件含生命周期钩子，开局读简报，适时保存 |
| Droid | Factory 插件，带工作记忆简报、路由式检索、提炼与可恢复的交接摘要 |
| Cursor | 插件包内含启动钩子、MCP 配置、路由式检索、提炼与可恢复交接摘要 |
| Claude Desktop | 一键安装的扩展，对话中随时保存、搜索、更新记忆 |
| Codex CLI | Codex 的 hybrid 路径：插件包负责 Working Memory、行为引导与真实会话保存，MCP 负责更主动的检索和记忆写入 |
| OpenCode | 原生插件：Working Memory、搜索、保存、会话捕获和可恢复交接的八个工具 |

如果你想在安装后确认这条路径是否真的已经生效，请配合[如何确认 Mem 已经在工作](https://mem.nowledge.co/zh/docs/verify-it-works)一起使用。

## LLM 友好文档

（API 集成文档部分）

## API 集成

RESTful API，完整访问你的知识库。

- API 参考 - Nowledge Mem RESTful API 完整文档。
- OpenAPI 规范 - `openapi.json`

## 命令行界面 (CLI)

`nmem` CLI 提供终端下的知识库访问，面向开发者和 AI 智能体。

### 安装

| 平台 | 安装方式 |
|-----|---------|
| macOS | 设置 → 偏好设置 → 开发者工具 → 安装 CLI |
| Windows | 随应用自动安装 |
| Linux | 包含在 deb/rpm 包中 |

### 快速上手

```bash
# 检查连接
nmem status
# 搜索记忆
nmem m search "项目笔记"
# 创建记忆
nmem m add "重要洞察" --title "项目学习"
# 保存 Claude Code/Codex/Gemini 会话
nmem t save --from claude-code
nmem t save --from codex -s "本次完成的内容摘要"
nmem t save --from gemini-cli -s "本次完成的内容摘要"
```

### AI 智能体集成

```bash
# JSON 输出，便于解析
nmem --json m search "API 设计"
# 链式命令
ID=$(nmem --json m add "笔记" | jq -r '.id')
nmem --json m update "$ID" --importance 0.9
```

### 命令参考

| 命令 | 别名 | 描述 |
|-----|------|-----|
| `nmem status` | | 检查服务器连接 |
| `nmem stats` | | 数据库统计 |
| `nmem memories` | `nmem m` | 记忆操作 |
| `nmem threads` | `nmem t` | 对话操作 |

完整文档：运行 `nmem --help` 或查看 GitHub 上的 [CLI 参考](https://github.com/nowledge-co/nowledge-mem/blob/main/nowledge-graph/docs/cli.md)。

---

展示你的集成：用 API 或 CLI 做了什么？在 [GitHub](https://github.com/nowledge-co/community/issues) 或 [Discord](https://nowled.ge/discord) 分享。

## 下一步

- [故障排除](https://mem.nowledge.co/zh/docs/troubleshooting)：常见问题和解决方案
- [后台智能](https://mem.nowledge.co/zh/docs/advanced-features)：知识图谱、洞察和自主功能
- [你的档案](https://mem.nowledge.co/zh/docs/profile)：Tell Mem who you are so your agents give better answers, more relevant labels, and more useful briefings


---

# 安全地自定义集成行为

让你的 Nowledge Mem 行为调整在插件或扩展升级后仍然保留。

如果你调整了集成行为，这个改动不应该在下次插件升级时消失。

最稳妥的规则很简单：**优先使用宿主自己提供的指令文件或设置入口，不要直接去改安装目录里的插件文件。**

## 第一步该做什么

先找到你正在使用的工具，把一条小规则写进下面对应的位置。之后正常升级一次插件，确认这条规则还在生效。

## 改在哪里最合适

| 工具 | 建议放在这里 | 适合什么场景 |
|-----|------------|------------|
| Codex CLI | 项目里的 `AGENTS.md` | 仓库级的记忆行为 |
| Claude Code | 个人规则放 `CLAUDE.local.md`，共享规则放 `CLAUDE.md` | 个人偏好或团队共享规则 |
| Copilot CLI | 仓库共享规则放 `.github/instructions/*.instructions.md`，个人规则放 `~/.copilot/instructions/*.instructions.md` | Copilot 的共享或个人行为 |
| Cursor | `.cursor/rules/*.mdc` 或 `.cursorrules` | 项目级 Cursor 规则 |
| Gemini CLI | 项目 `GEMINI.md`，可选 `~/.gemini/GEMINI.md` | 仓库规则或个人默认规则 |
| Hermes Agent | 仓库规则放 `HERMES.md`，个人规则放 `~/.hermes/SOUL.md` | 项目级或全局 Hermes 行为 |
| OpenCode | 项目 `AGENTS.md`、`~/.config/opencode/AGENTS.md`，或 `opencode.json` 里的 `instructions` 文件 | OpenCode 的共享或个人行为 |
| Pi | 项目 `AGENTS.md` | 项目级 Pi 行为 |
| OpenClaw | OpenClaw 插件设置，以及 OpenClaw 自己的提示词或 Agent 配置 | 行为开关和额外提示 |
| Alma | Alma 设置，必要时再手动加载额外的 skill 提示 | 回忆、捕获策略和补充指令 |
| Bub | Bub 自己的运行提示或配置，再配合共享的 `nmem` 配置 | Bub 的行为和连接方式 |
| Droid | Droid 自己的提示词或指令入口，再配合共享的 `nmem` 配置 | Droid 的行为和连接方式 |
| Raycast | Raycast 偏好设置 | 启动器层面的固定行为，比如服务器地址、密钥、space |

## 不要这样做

- 不要直接修改安装目录里的插件文件，比如 `~/.codex/...`、`~/.copilot/installed-plugins/...`、`~/.cursor/plugins/...` 这些缓存路径。
- 不要在原地修改插件自带的 skills，然后期待升级后还能保留。
- 如果宿主根本不会读取某个文件名，就不要自己发明一个新的"覆盖文件"名字。

## 什么才算真正配好了

满足下面三点，就说明这条路径是稳的：
1. 你的自定义规则放在宿主真正会读取的位置。
2. 插件或扩展正常升级后，集成本身还能工作。
3. 升级后不需要再去改安装目录，你的行为调整依然生效。

## 很适合先加的一条小规则

- "遇到回归问题时，先搜索以前的发布或修复记录。"
- "用户用中文工作时，更倾向保存中文记忆。"
- "当用户问'我们之前怎么决定的'时，更主动地搜索历史对话。"

规则越短越好。目标是轻轻地调整行为，而不是重写整套集成说明。

## 下一步

- [集成概览](https://mem.nowledge.co/zh/docs/integrations)
- [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli)
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)
- [Copilot CLI](https://mem.nowledge.co/zh/docs/integrations/copilot-cli)


---

# 浏览器扩展

通过 Nowledge Mem Exchange 浏览器扩展，从受支持的 Web AI 聊天平台捕获记忆与对话备份。

Nowledge Mem Exchange 是一款浏览器扩展，可从受支持的 Web AI 聊天平台捕获记忆与对话备份，在 Chrome 侧边栏中与对话并排运行。

**第一次成功应该是什么样**

打开一个受支持的网站，再打开侧边栏，只做一件事就够了：自动捕获一条洞察、手动提炼一段对话，或备份一条线程。只要对应结果出现在 Mem 里，这条路径就已经接通。

## 智能提炼

现在，扩展不会再把整段对话粗暴打包后一次性丢给后端。无论是自动捕获还是手动提炼，它都会把当前对话当成一个可以"按需阅读"的线程：先看哪里重要，再查 Mem 里已有的内容，最后决定是创建新记忆还是补充已有记忆。

## 三种捕获方式

| 模式 | 工作方式 | 适用场景 |
|-----|---------|---------|
| **自动捕获** | 持续监控对话，自主保存有价值的洞察 | 配置好就不用管。扩展自行判断什么值得记 |
| **手动提炼** | 由你触发对特定对话的捕获 | 当你知道这次对话有重要内容时 |
| **对话备份** | 将完整对话作为线程导入，支持增量去重 | 归档整个对话，稍后在应用中提炼 |

### 自动捕获

启用后，扩展会持续观察当前对话，并以很高的门槛判断是否值得保存：

- **精炼的结论**：决策、计划、最终方案
- **重要发现**：突破性进展、关键洞察
- **知识探索**：深度研究、综合分析

日常问答和泛泛交流会被跳过。真正准备写入前，扩展会先更仔细地查看线程内容，再去 Mem 中检查是否重复；如果已经有相关记忆，它会优先更新，而不是再造一条相似内容。

如果自动捕获真的写入了记忆，它还会先把当前对话保存成 Mem 中的标准线程，再把记忆挂到这条线程上。这样以后回看时，你能知道这条记忆到底来自哪次真实对话。

### 手动提炼

手动提炼适合那种"我知道这段对话很重要，值得认真整理"的时刻。

现在它不再只是把整段对话压成一个粗略摘要，而是会：
- 按需阅读当前线程中真正重要的部分
- 重点寻找动机、约束、决策、权衡和可复用背景
- 先检查 Mem 中是否已有相关知识
- 在更合适时，一次对话中创建或更新多条记忆

这让它更适合长讨论、设计推演、调试过程，以及那些"最重要的信息其实不在最后一句"的对话。

**需要 LLM**

自动捕获需要配置 LLM 提供商。打开侧边栏，进入**设置**，添加 API 密钥。支持：OpenAI、Anthropic、Google、xAI、OpenRouter、Ollama 以及 OpenAI 兼容端点。

### 对话备份

你仍然可以随时把完整对话显式备份成线程，后续备份只会导入新增消息（增量同步）。

另外，当扩展真的从浏览器对话里写入新记忆时，它也会自动先保存标准线程。这样你不需要手动备份每一段对话，也依然能保留清晰的来源关系。

## 支持的平台

| 平台 | URL |
|-----|-----|
| ChatGPT | chatgpt.com |
| Claude | claude.ai |
| Gemini | gemini.google.com |
| Microsoft Copilot | copilot.microsoft.com |
| Perplexity | perplexity.ai |
| Poe | poe.com |
| DeepSeek | chat.deepseek.com |
| Grok | grok.com |
| Kimi | kimi.moonshot.cn |
| Qwen | tongyi.aliyun.com |
| Doubao | doubao.com |
| Coze | coze.com |
| Yuanbao | yuanbao.tencent.com |
| ChatGLM | chatglm.cn |
| MiniMax | agent.minimaxi.com |
| Manus | app.manus.im |
| Open WebUI | + many more |

**不支持的网站？**

配置了 LLM 的 Pro 用户可以为任意 AI 聊天网站自动生成处理器。打开侧边栏，点击**生成处理器**，扩展会自动分析页面并生成处理器。

## 接入「随处访问 Mem」

如果你已通过**设置 → 随处访问 Mem** 暴露了 Mem API：

1. 打开任意受支持的 AI 对话页面，打开扩展侧边栏
2. 点击 **Settings**
3. 只有当这个浏览器 profile 应该长期停留在一个命名好的 lane 时，才填写 **Fixed Space**。留空就继续使用 **Default**。
4. 在 **Access Mem Anywhere** 中粘贴：
   ```bash
   export NMEM_API_URL="https://<your-url>"
   export NMEM_API_KEY="nmem_..."
   ```
5. 点击 **Fill URL + key**
6. 点击 **Save**，再点击 **Test connection**

## Spaces

浏览器扩展适合的空间模型，是"一个固定 lane"：
- 如果这个浏览器 profile 继续用 **Default**，就把 **Fixed Space** 留空
- 如果这个 profile 天然就只属于一个上下文，再填写一个命名好的空间
- 不要期待扩展自己从网页里推断"当前 agent 属于哪个 lane"

当你设置了固定空间后，扩展中的记忆搜索、记忆写入、线程读取、来源读取和 Working Memory 读取都会跟着这个 lane 走。

完整流程：[随处访问 Mem](https://mem.nowledge.co/zh/docs/remote-access)。

## 下载

- **Chrome** - Browser extension to capture conversations from Web AI chat services. [Get Extension]
- **Edge** - Browser extension to capture conversations from Web AI chat services. [Get Extension]

扩展还支持将任意对话线程下载为 `.md` 文件，用于归档或分享。

## 下一步

- [记忆](https://mem.nowledge.co/zh/docs/memories)：扩展捕获的内容。创建、搜索和组织你的知识
- [对话](https://mem.nowledge.co/zh/docs/threads)：对话备份和提炼工作流
- [随处访问 Mem](https://mem.nowledge.co/zh/docs/remote-access)：从远程设备连接扩展
- [安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)：Keep your Nowledge Mem behavior tweaks across plugin and extension updates.


---

# Claude Code

Claude Code 专属插件，内置生命周期钩子。会话开始时自动读取工作记忆简报，并在恰当时机搜索与保存。

## 一键安装

```bash
claude plugin marketplace add nowledge-co/community && claude plugin install nowledge-mem@nowledge-community
```

插件原生支持 Claude Code 支持插件，一次安装即可获得内置自主行为，无需配置系统提示或 MCP。

智能体会自动搜索你已经知道的内容，并在正确的时机保存值得留下的东西，无需你手动触发。

**第一次成功应该是什么样**

安装好插件、确认 `nmem` 可用，然后开一个新的 Claude Code 会话。只要你能看到工作记忆简报在开局被读取，或者 `/search` / `/save` 无需额外配置就能工作，这条路径就已经接通了。

## 开始之前

- Nowledge Mem 已在本地运行（[安装指南](https://mem.nowledge.co/zh/docs/installation)），或你已经有可访问的远程 Mem 服务
- 已安装 Claude Code

## 设置

### 安装插件

```bash
# 添加 Nowledge 社区插件市场
claude plugin marketplace add nowledge-co/community
# 安装 Nowledge Mem 插件
claude plugin install nowledge-mem@nowledge-community
```

插件需要 `nmem` CLI：

```bash
# 方式一（推荐）：使用 uvx，无需额外安装
curl -LsSf https://astral.sh/uv/install.sh | sh
uvx --from nmem-cli nmem --version
# 方式二：pip 安装
pip install nmem-cli
```

在 Windows/Linux 上安装了 Nowledge Mem 桌面应用时，`nmem` 已内置。macOS 或远程服务器上请使用 `uvx` 或手动安装。

### 使用斜杠命令与技能

**斜杠命令：**

（具体命令列表请参考页面内容）

## 安全地自定义

个人调整放在 `CLAUDE.local.md`
仓库共享规则放在 `CLAUDE.md`

不要直接修改已安装的 Nowledge Mem 插件文件。完整对照表见[安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## 生命周期钩子

插件使用 Claude Code hooks 实现自动生命周期管理：

| 事件 | 触发条件 | 操作 |
|-----|---------|------|
| SessionStart（启动/恢复/清除） | 新会话、恢复或清除 | 通过 `nmem wm read`（API）加载工作记忆简报，回退到本地文件 |
| SessionStart（压缩） | 上下文压缩后 | 重新加载工作记忆简报，提示 Claude 保存重要发现 |
| UserPromptSubmit | 每条用户消息 | 注入搜索/保存语法提示，Claude 可见 |
| Stop | 模型完成响应 | 异步捕获会话到知识图谱（幂等） |

工作记忆简报会始终存在于上下文中。

Stop 钩子确保即使 Nowledge Mem 运行在不同机器上（远程模式），会话也能被捕获。

## 本地与远程模式

插件在两种模式下透明运行：

**本地**（Mem 在同一台机器上）：优先通过 Mem 读取 Working Memory。只有 API 路径不可用时，插件才会把 `~/ai-now/memory.md` 当作 Default 分区的兼容回退。会话仍由桌面应用文件监听和 Stop 钩子双重捕获。

**远程**（Mem 在不同机器上）：先在这台机器上执行一次：

```bash
nmem config client set url https://your-server:14242
nmem config client set api-key your-key
```

这会写入 `nmem` 和插件共用的本地客户端配置。也可使用环境变量（`NMEM_API_URL`、`NMEM_API_KEY`）做临时覆盖。优先级：CLI 参数 > 环境变量 > 配置文件 > 默认值。

当 Stop 钩子运行 `nmem t save --from claude-code` 时，Claude 的会话文件会先在运行 Claude Code 的那台机器上被本地读取，再以规范化后的线程消息上传到 Mem。默认会读取 `~/.claude`；如果你把 Claude 配置放在别的位置，设置 `CLAUDE_CONFIG_DIR` 就可以了。远程 Mem 服务器不需要直接访问这个本地目录。

## 进阶

- [AGENTS.md](https://github.com/nowledge-co/community/blob/main/examples/AGENTS.md) -- 基于 [agents.md 标准](https://agents.md/) 的完整记忆守护 Agent 示例，适配任何 AI 编程工具。

## 相关

- [集成概览](https://mem.nowledge.co/zh/docs/integrations) -- 原生集成、复用包、MCP 与浏览器捕获
- [Claude Desktop](https://mem.nowledge.co/zh/docs/integrations/claude-desktop) · [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli) · [Alma](https://mem.nowledge.co/zh/docs/integrations/alma) · [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw) · [Raycast](https://mem.nowledge.co/zh/docs/integrations/raycast) · [内置 Web 聊天](https://mem.nowledge.co/zh/docs/integrations/built-in-web-chats)


---

# Claude Desktop

一个适用于 macOS 和 Windows 的 Claude Desktop 一键扩展。在任意对话中保存洞察、搜索记忆、更新知识库。

在 macOS 和 Windows 上装一次就能用。不需要额外安装 Python 或 Node。之后 Claude Desktop 可以在任意对话里直接搜索记忆、保存新内容、更新已有知识。

**安装成功后应该看到什么**

安装完成后，在 Claude Desktop 对话框左下角点击 `+`，打开 **Connectors**。如果能看到 **Nowledge Mem**，这条连接就已经通了。

## 开始之前

- 这台机器上已经运行了 Nowledge Mem（[安装指南](https://mem.nowledge.co/zh/docs/installation)），或者你已经有可访问的远程 Mem 服务
- 已安装并更新到最新 Claude Desktop

## 设置

### 下载扩展

[下载扩展](https://nowled.ge/claude-dxt)

### 安装并重启 Claude Desktop

双击下载好的 `claude-dxt.mcpb` 文件，在弹窗里点击**安装**，然后重启一次 Claude Desktop。

### 在对话中使用 Mem

安装完成后，在任意对话中就可以使用 Mem 的能力来保存洞察、搜索记忆、更新知识库。

## 随处访问 Mem

扩展默认连接本机 Mem。要连接远程 Mem，它会直接读取和 `nmem` CLI 相同的共享客户端配置。

如果你是在同一台机器上的 Nowledge Mem 桌面端里开启**随处访问 Mem**，通常不需要再额外配置。桌面端会自动把连接信息写进去。

如果 Claude Desktop 跑在另一台机器上，先在那台机器上完成配置：

```bash
nmem config client set url https://mem.example.com
nmem config client set api-key nmem_your_key
```

这会写入 Claude Desktop 读取的那份共享客户端配置。如果你想手动查看或编辑文件，路径如下。

路径：`~/.nowledge-mem/config.json`

`apiUrl` 建议填写你的服务器根地址。如果你手里已有旧配置，结尾是 `/remote-api` 或 `/mcp` 也可以继续用，扩展会自动兼容。

改完连接配置后**重启 Claude Desktop**。

## 排查问题

如果 Claude Desktop 里没有出现 Mem：
- 点击对话框左下角的 `+`，打开 **Connectors**，确认里面能看到 **Nowledge Mem**。
- 打开 **Settings → Extensions → Advanced Settings**，查看扩展状态和日志。
- 如果你走的是远程连接，在那台机器上运行 `nmem config client show`，确认 URL 和 API Key 状态都正确。

## 相关

- **集成概览**：原生集成、复用包、MCP 与浏览器捕获
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code) · [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli) · [Alma](https://mem.nowledge.co/zh/docs/integrations/alma) · [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw) · [Raycast](https://mem.nowledge.co/zh/docs/integrations/raycast) · [内置 Web 聊天](https://mem.nowledge.co/zh/docs/integrations/built-in-web-chats)


---

# Droid

通过社区 marketplace 在 Factory Droid 中安装 Nowledge Mem，把工作记忆简报、路由式检索、提炼和可恢复交接带进你的会话。

## 推荐安装路径

先把 Nowledge 社区 marketplace 加到 Droid，再安装 `nowledge-mem@nowledge-community`，运行一次 `nmem status`，然后开始新的 Droid 会话。

**这个包当前提供什么**

Droid 会获得原生插件入口：工作记忆简报、路由式检索、提炼、状态检查和可恢复交接摘要。它现在**不会**声称自己支持 `save-thread`。

对 Nowledge Mem 来说，Droid 是很合适的宿主。Factory 插件可以把 hooks、commands 和 skills 收敛成一个统一入口，而底层的记忆执行仍然交给 `nmem`。

## 开始之前

- Nowledge Mem 已在本地运行（[安装指南](https://mem.nowledge.co/zh/docs/installation)），或你已经有可访问的远程 Mem 服务
- 已安装 Factory Droid
- `nmem` 在你的 PATH 中

如果你已经在同一台机器上运行 Nowledge Mem 桌面应用，最省事的方式仍然是 **Settings -> Preferences -> Developer Tools -> Install CLI**。这样 Droid 就能直接调用 `nmem`，本地和远程都走同一套路径。

你也可以单独安装 `nmem`：

```bash
# 方式一：pip
pip install nmem-cli
# 方式二：uvx
curl -LsSf https://astral.sh/uv/install.sh | sh
uvx --from nmem-cli nmem --version
```

## 一分钟安装

### 添加 Nowledge 社区 marketplace

```bash
droid plugin marketplace add https://github.com/nowledge-co/community
```

### 安装插件

```bash
droid plugin install nowledge-mem@nowledge-community
```

### 运行一次 nmem status

```bash
nmem status
```

### 开始新的 Droid 会话

开启新的 Droid 会话，插件将自动生效。

## 更新

后续更新时，再次运行：

```bash
droid plugin update nowledge-mem@nowledge-community
```

## 安全地自定义

Droid 的自定义规则放在 Droid 自己的提示词或指令入口，再配合共享的 `nmem` 配置。完整对照表见[安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## 你会得到什么

- 工作记忆简报（Working Memory）读取
- 路由式检索（搜索记忆）
- 记忆提炼
- 状态检查
- 可恢复交接摘要

## 命令

| 命令 | 描述 |
|-----|------|
| `/nowledge-read-working-memory` | 读取工作记忆简报 |
| `/nowledge-search-memory` | 搜索记忆 |
| `/nowledge-distill-memory` | 提炼记忆 |
| `/nowledge-save-handoff` | 保存交接摘要 |
| `/nowledge-status` | 检查状态 |

## 远程 Mem

推荐的远程配置方式是：

```bash
nmem config client set url https://mem.example.com
nmem config client set api-key nmem_your_key
```

这会写入这台机器上的共享客户端配置。这样 Droid 会和 Gemini、Codex、CLI 等路径保持同一套本地/远程契约。

## 重要约束

这个包刻意只暴露 `save-handoff`，不暴露 `save-thread`。

这里的边界必须保持清楚：
- `save-thread` 应该表示导入真实录制下来的整段会话
- `save-handoff` 表示保存一个可恢复的交接摘要

Droid 现在还没有真正的 Nowledge transcript 导入器，所以这个插件不会把摘要伪装成完整会话保存。

## 本地仓库回退路径

如果你更想先从本地 checkout 验证：

```bash
git clone https://github.com/nowledge-co/community.git
cd community
droid plugin marketplace add .
droid plugin install nowledge-mem@nowledge-community
```

之所以这样可行，是因为 `community` 仓库在根目录发布了 Factory marketplace 清单，而 Droid 插件则作为其中一个独立包存在。

## 相关内容

- [集成总览](https://mem.nowledge.co/zh/docs/integrations)
- [对话](https://mem.nowledge.co/zh/docs/threads)
- [Cursor](https://mem.nowledge.co/zh/docs/integrations/cursor)
- [Gemini CLI](https://mem.nowledge.co/zh/docs/integrations/gemini-cli)
- [远程访问](https://mem.nowledge.co/zh/docs/remote-access)


---

# Cursor

现在就可以通过 Cursor 本地插件目录安装 Nowledge Mem，以后如果 Marketplace 可见，再切换过去。

## 今天最稳的路径：本地插件安装

现在最可靠的路径是把插件放到 `~/.cursor/plugins/local/nowledge-mem-cursor`。即使 Marketplace 还没通过，这条路也可以直接给用户使用。

**Marketplace 以后再说**

如果将来 **Nowledge Mem** 出现在你的 Cursor Marketplace 账号里，你再切过去即可。下面这条本地安装路径才是现在面向用户的真实可用路径。

对用户来说，最重要的好处很直接：**装一次，开一个新的智能体会话，规则、技能、启动钩子和 MCP 连线就一起到位了。**

## 开始之前

- Nowledge Mem 已在本地运行（[安装指南](https://mem.nowledge.co/zh/docs/installation)），或你已经有可访问的远程 Mem 服务
- 已安装 Cursor IDE
- 推荐：如果你希望在会话开始时自动带上 Working Memory，并在需要时创建可恢复交接摘要，请让 `nmem` 出现在 PATH 中

如果 Nowledge Mem 已经在同一台机器上的桌面应用中运行，最省事的方式仍然是 **Settings -> Preferences -> Developer Tools -> Install CLI**。这样 Cursor 就可以在会话开始时加载 Working Memory，也能在需要时通过终端工具调用 `nmem`。

## 一分钟安装

### 克隆或进入插件仓库

```bash
git clone https://github.com/nowledge-co/community.git
cd community
```

### 复制到 Cursor 的本地插件目录

```bash
mkdir -p ~/.cursor/plugins/local
rm -rf ~/.cursor/plugins/local/nowledge-mem-cursor
cp -R nowledge-mem-cursor-plugin ~/.cursor/plugins/local/nowledge-mem-cursor
```

如果你为了本地迭代想试软链接，也可以：

```bash
ln -s "$(pwd)/nowledge-mem-cursor-plugin" ~/.cursor/plugins/local/nowledge-mem-cursor
```

但 Cursor 团队已经确认，本地插件的软链接解析现在实际还不稳定，所以面向用户时应优先用复制方案。

### 重新加载 Cursor

重新加载 Cursor 使插件生效。

### 开启新的智能体会话

开一个新的 Cursor 智能体会话，插件会自动加载 Working Memory 和相关规则。

### 仅在远程 Mem 时配置 MCP

这里有两个独立通道：
- Cursor 的 MCP 设置负责插件里的记忆工具
- `nmem config client ...` 负责这台机器上终端侧的启动简报和交接摘要能力

## 更新

如果你走的是本地插件目录：

- **推荐复制方式**：用新的 `nowledge-mem-cursor-plugin` 覆盖 `~/.cursor/plugins/local/nowledge-mem-cursor`，然后重新加载 Cursor
- **可选软链接方式**：更新 `community` 仓库后重新加载 Cursor；如果 Cursor 又不识别插件，就切回复制方式

如果以后 Marketplace 可用了，再从本地版本切过去即可。

## 安全地自定义

想调整行为时，优先使用 Cursor 自己的规则文件：
- `.cursor/rules/*.mdc`
- `.cursorrules`

Nowledge Mem 插件自带的规则应继续作为默认值保留，不要直接去改 `~/.cursor/plugins/...` 里的已安装插件文件。完整对照表见[安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## 第一次成功应该看到什么

当下面几件事成立时，就说明插件已经接通了：
- 你已经把插件放进 `~/.cursor/plugins/local/nowledge-mem-cursor`
- 你重新打开了一个新的 Cursor 智能体会话
- 如果装了 `nmem`，一开始就能看到 Working Memory 已经进了上下文
- 同机默认配置下，不需要手改 MCP 就能工作
- 远程模式下，只要改好 `nowledge-mem` MCP 服务器地址，就能恢复同样的行为
- Cursor 里不再出现旧的 Claude 风格包界面，比如 `save-thread`、`beforeSubmitPrompt` 或 `stop`

## 你会得到什么

- 打包好的 `.cursor-plugin/plugin.json`
- 用于启动时加载 Working Memory 的 `hooks/hooks.json`
- 用于本地 Nowledge Mem MCP 连接的 `mcp.json`
- 一条常驻规则，用来约束工作记忆简报、路由式检索、提炼与交接摘要语义
- 四个技能：`read-working-memory`、`search-memory`、`distill-memory` 与 `save-handoff`

## 新用户真正关心的事

对第一次安装的用户来说，最重要的其实只有三件事：
1. **本地 Mem**：装好插件后直接开新会话
2. **远程 Mem**：装好插件后，把 `nowledge-mem` MCP 服务器改到远程地址；如果还想保留启动简报和交接摘要，再把 `nmem` 也指向同一个远程服务
3. **想把体验拉满**：保留 `nmem` CLI，让 Cursor 能在新会话开始时先带上 Working Memory，也能在需要时生成交接摘要

## 重要约束

这个包故意**不**暴露 `save-thread`。

因为 Nowledge Mem 目前还没有在这里提供 Cursor 的一等实时会话导入器，所以摘要型保存必须继续叫 `save-handoff`，不能伪装成 `save-thread`。如果你需要导入真实 Cursor 对话，请使用应用里的**对话 -> 导入 -> 查找 AI 对话**。本页里的插件语义必须保持清楚，未来真正的会话线程保存能力出现时才不会混乱。

## 包结构

这个包已经按 Cursor 插件格式组织好：

```
.cursor-plugin/plugin.json
rules/nowledge-mem.mdc
skills/*/SKILL.md
hooks/hooks.json
hooks/session-start.mjs
mcp.json
```

Cursor 的本地插件目录要求 `.cursor-plugin/plugin.json` 位于插件根目录。这个包已经满足这个约定，所以前面的本地安装流程就是现在可以直接给用户使用的正式路径。

## 以后如果上架 Marketplace

如果未来 Cursor Marketplace 接受了这个插件，再安装 Marketplace 版本即可。上架前，优先使用本地插件目录。

## 相关内容

- [集成总览](https://mem.nowledge.co/zh/docs/integrations)
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)
- [Gemini CLI](https://mem.nowledge.co/zh/docs/integrations/gemini-cli)
- [远程访问](https://mem.nowledge.co/zh/docs/remote-access)
- [Droid](https://mem.nowledge.co/zh/docs/integrations/droid)
- [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli)



---

# 第四部分：更多工具集成

# Codex CLI

让你的 Codex 智能体记住过往决策、查找相关上下文，并从每次会话中积累经验，跨工具延续记忆。

## Codex 复用型记忆包

现代 Codex 的推荐做法是：插件 + Nowledge Mem MCP 一起装。插件负责 Working Memory、真实线程保存和 `nmem` 兜底；MCP 会让 Codex 更愿意主动去检索和写入记忆。

在 Codex、Claude Code、Gemini、Cursor 之间切换，不丢失上下文。这个 Codex 包会稳定完成 Working Memory 启动，然后把搜索、蒸馏、保存能力作为技能交给 Codex 使用。实际体验里，现代 Codex 往往比起 skill-only 引导，更愿意直接调用 MCP 工具，所以现在推荐的组合是 hybrid：插件 + MCP。

### 如何确认安装成功

开始一次会话，先问「我在做什么？」你应该看到最近的工作重点和优先事项。然后再问一个 continuation 型问题，比如「我们之前对这个发布流程做过什么决定？」正常情况下，Codex 不应该停在简报这里，而会继续进入检索。你也可以运行 `$nowledge-mem:status` 检查 CLI 侧连接是否正常。

## 开始之前

- Nowledge Mem 已在本地运行（[安装指南](https://mem.nowledge.co/zh/docs/installation)），或有可访问的远程 Mem 服务
- 已安装 Codex CLI
- `nmem` CLI 在你的 `PATH` 中

## 设置

### 1. 安装 nmem

**方式一：uvx**

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
uvx --from nmem-cli nmem --version
```

**方式二：pip**

```bash
pip install nmem-cli
```

如果 Nowledge Mem 桌面应用已在同一台机器上运行，推荐的方式是 Settings → Preferences → Developer Tools → Install CLI。

### 2. 安装插件

**优先使用 marketplace 安装**

如果你看到旧文档让你手动复制文件到 `~/.codex/plugins/cache/local/...`，请把它当作兼容旧流程。当前推荐路径是：先添加 marketplace，再到 Codex 的 `/plugins` 安装插件，最后在配置里启用。

```bash
codex plugin marketplace add nowledge-co/community
```

如果你的 Codex 还是旧版顶层子命令：

```bash
codex marketplace add nowledge-co/community
```

打开 Codex 的 `/plugins`，安装 `nowledge-mem@nowledge-community`。

### 3. 添加 Nowledge Mem MCP 服务器

把下面这段放进 `~/.codex/config.toml`，这是推荐的本地配置：

```toml
[features]
plugins = true

[plugins."nowledge-mem@nowledge-community"]
enabled = true

[mcp_servers.nowledge-mem]
url = "http://127.0.0.1:14242/mcp/"

[mcp_servers.nowledge-mem.http_headers]
APP = "Codex"
```

安装完成后重启 Codex。

**为什么这里要多加一步 MCP**

只装插件当然也有价值：它仍然提供 Working Memory 引导、真实 `save-thread`、`nmem` 兜底，以及项目级 `AGENTS.md` 行为约束。但如果你希望 Codex 更主动地去搜索历史知识、读取旧线程、写入记忆，直接把 Nowledge Mem MCP 工具暴露给它，效果通常更好。

### 4. 可选：添加项目级引导

将插件的 `AGENTS.md` 复制或合并到你的项目根目录，增强该仓库中的记忆行为：

```bash
git clone https://github.com/nowledge-co/community.git /tmp/nowledge-community
cp /tmp/nowledge-community/nowledge-mem-codex-plugin/AGENTS.md ./AGENTS.md
rm -rf /tmp/nowledge-community
```

如果你的项目已经有 `AGENTS.md`，请把 Nowledge 部分合并进去，而不是直接覆盖。这一步能明显改善 continuation-heavy 仓库里只读 Working Memory、不继续搜索的情况。

> **不要修改已安装插件里的文件**
> 把你仓库自己的 `AGENTS.md` 当作长期 override 层。插件包里的 `AGENTS.md` 只是参考文本，用来复制或合并，不要直接去改 Codex 插件安装目录里的那份。

### 5. 需要远程 Mem 时配置

```bash
nmem config client set url https://mem.example.com
nmem config client set api-key nmem_your_key
```

`nmem` 的连接优先级：

1. `--api-url` / `--api-key` 参数
2. `NMEM_API_URL` / `NMEM_API_KEY` 环境变量
3. `~/.nowledge-mem/config.json` 默认值

如果 Mem 在远程机器上，也要把同一个 `~/.codex/config.toml` 里的 MCP 配置指向远程地址：

```toml
[mcp_servers.nowledge-mem]
url = "https://mem.example.com/mcp/"

[mcp_servers.nowledge-mem.http_headers]
APP = "Codex"
Authorization = "Bearer nmem_your_key"
```

这段 MCP 配置和本机 `nmem` 客户端配置，应该一起指向同一台 Mem 服务器。MCP 给 Codex 直接工具，`nmem` 继续负责插件里的兜底行为和真实 `save-thread`。

## 项目级安装（可选方案）

除了共享 marketplace 源，也可以把插件打包进项目仓库，通过本地 Codex marketplace 文件让 Codex 自动发现。这样克隆仓库的人可以直接使用。

```bash
git clone https://github.com/nowledge-co/community.git /tmp/nowledge-community
mkdir -p .agents
cp -r /tmp/nowledge-community/nowledge-mem-codex-plugin ./.agents/nowledge-mem
rm -rf /tmp/nowledge-community
mkdir -p .agents/plugins
```

创建 `.agents/plugins/marketplace.json`：

```json
{
  "name": "local",
  "plugins": [
    {
      "name": "nowledge-mem",
      "source": {
        "source": "local",
        "path": "./.agents/nowledge-mem"
      },
      "policy": {
        "installation": "INSTALLED_BY_DEFAULT"
      }
    }
  ]
}
```

`path` 相对于仓库根目录，而非 marketplace 文件本身。这个本地方案需要使用：

```toml
[plugins."nowledge-mem@local"]
enabled = true
```

## 更新

```bash
codex plugin marketplace update nowledge-community
```

如果你的 Codex 不支持 `plugin marketplace update`，请改用：

```bash
codex plugin marketplace upgrade nowledge-community
```

如果这两个更新命令都没有，再改用：

```bash
codex marketplace add nowledge-co/community
```

然后重启 Codex。如果你用的是项目内 `@local` 源，请更新本地源路径。

## 技能

在 hybrid 配置里，这些技能仍然重要。它们负责告诉 Codex 什么时候该用记忆，而 MCP 负责在 Codex 决定行动时，给它一个更顺手的执行入口。

| 技能 | 触发条件 | 功能 |
|------|----------|------|
| `$nowledge-mem:working-memory` | 会话开始、"我在做什么" | 读取当天的工作记忆简报；如果有 MCP，就优先走 `read_working_memory` |
| `$nowledge-mem:search-memory` | 涉及过往工作、过去的决策 | 搜索记忆和对话，支持逐层深入查看；如果有 MCP，就优先走检索工具 |
| `$nowledge-mem:save-thread` | "保存这次会话" | 通过 `nmem t save --from codex` 导入真实 Codex 会话 |
| `$nowledge-mem:distill-memory` | 做出决策、发现经验教训 | 主动将有价值的洞察保存为记忆；如果有 MCP，就优先走写入工具 |
| `$nowledge-mem:status` | "Mem 能用吗"、出错时 | 检查服务器连接和配置状态 |

## 直接使用 nmem

`nmem` 仍然是通用兜底层，也是 Codex 真实线程导入的唯一正确路径：

```bash
nmem --json wm read
nmem --json m search "auth token rotation" --mode deep
nmem --json t save --from codex -p . -s "完成了 auth 重构"
nmem --json m add "JWT 刷新失败源于时钟偏移" --title "JWT 刷新失败追溯到时钟偏移" --importance 0.9 --unit-type learning -l auth -s codex
```

默认情况下，`nmem t save --from codex` 会去 `~/.codex` 里找会话。如果 Codex Home 在其他位置，设置 `CODEX_HOME` 即可。

## 从自定义提示词迁移

如果之前使用的是 `nowledge-mem-codex-prompts`，这个插件完整覆盖了原有功能：

1. 安装插件（见上方步骤）。
2. 删除旧提示词：`rm ~/.codex/prompts/{read_working_memory,search_memory,save_session,distill}.md`
3. 插件技能一一对应替代旧提示词。

| 旧提示词 | 新技能 |
|----------|--------|
| `/prompts:read_working_memory` | `$nowledge-mem:working-memory` |
| `/prompts:search_memory` | `$nowledge-mem:search-memory` |
| `/prompts:save_session` | `$nowledge-mem:save-thread` |
| `/prompts:distill` | `$nowledge-mem:distill-memory` |
| （无） | `$nowledge-mem:status` |

## 常见问题

**找不到 nmem 命令**

用 `pip install nmem-cli` 安装，或使用 `uvx --from nmem-cli nmem`。参见[安装指南](https://mem.nowledge.co/zh/docs/installation)。

**无法连接服务器**

运行 `nmem status` 和 `nmem config client show` 检查远程配置是否正确。参见[远程访问](https://mem.nowledge.co/zh/docs/remote-access)。

**技能没有出现**

安装插件后需重启 Codex。确认三件事：已经添加 marketplace、已经在 `/plugins` 安装 `nowledge-mem@nowledge-community`，以及 `~/.codex/config.toml` 中同时包含 `[features] plugins = true` 和 `[plugins."nowledge-mem@nowledge-community"] enabled = true`。如果你是项目内本地源方案，使用 `[plugins."nowledge-mem@local"]`。

**显示 "plugin is not installed"**

先运行 `codex plugin marketplace add nowledge-co/community`（旧版 Codex 用 `codex marketplace add nowledge-co/community`），再到 `/plugins` 安装 `nowledge-mem@nowledge-community`，然后检查 `~/.codex/config.toml` 里的插件 key 是否正确。

**只会读取 Working Memory，不继续搜索或蒸馏**

这通常说明 Codex 现在只看到了插件侧最稳定的启动动作。先把 Nowledge Mem MCP 服务器加进 `~/.codex/config.toml`，再把插件里的 `AGENTS.md` 合并到项目根目录。只装插件时，Working Memory 会比较稳，但主动检索通常会弱很多。

## 相关内容

- [集成总览](https://mem.nowledge.co/zh/docs/integrations)
- [Gemini CLI](https://mem.nowledge.co/zh/docs/integrations/gemini-cli)
- Claude Code
- 远程访问
- Cursor

---

# Copilot CLI

GitHub Copilot CLI 的专属插件路径，提供 Working Memory 启动、检索引导，以及增量会话捕获到 Mem。

## 一分钟安装

```bash
copilot plugin marketplace add nowledge-co/community
copilot plugin install nowledge-mem@nowledge-community
```

## 原生插件路径

Copilot CLI 支持插件和生命周期钩子。安装一次插件、确保本地能调用 `nmem`，Copilot 就可以在开局读取 Working Memory，在需要时搜索过去上下文，并把会话持续追加进 Mem。

这是 GitHub Copilot CLI 对应的专属 Nowledge 路径。它比通用 MCP 更完整：插件自带 hooks、skills 和会话捕获运行时。但它又不像 OpenClaw 那样几乎把行为都塞进宿主生命周期里，Copilot 仍然需要依赖模型去决定何时调用技能。所以如果你希望它更主动地检索和蒸馏，项目里的 `AGENTS.md` 依然很重要。

### 怎样算安装成功

启动一个新的 Copilot 会话，问一句 `What was I working on?`。如果能看到最近的重点和优先事项，再让 Copilot 保存或 checkpoint 当前会话，并确认这条线程出现在 Mem 里，这条路径就接通了。

## 开始之前

- Nowledge Mem 已在本地运行（[安装指南](https://mem.nowledge.co/zh/docs/installation)），或你已经有可访问的远程 Mem 服务
- 已安装 GitHub Copilot CLI
- `nmem` CLI 在你的 `PATH` 中

## 设置

### 1. 安装 nmem

```bash
# 方式一：使用 Mem 桌面应用附带的 CLI
# Settings -> Preferences -> Developer Tools -> Install CLI

# 方式二：pip
pip install nmem-cli

# 方式三：pipx
pipx install nmem-cli
```

然后先确认：

```bash
nmem status
```

如果你在 Windows 或 Linux 上使用 Nowledge Mem 桌面应用，`nmem` 一般已经随应用提供。

### 2. 安装插件

```bash
copilot plugin marketplace add nowledge-co/community
copilot plugin install nowledge-mem@nowledge-community
```

安装完成后重启 Copilot CLI。

插件现在会直接从自己打包好的 `hooks/` 目录运行会话捕获。如果你本地已经有旧版留下的兼容副本，也仍然可以回退到 `~/.copilot/nowledge-mem-hooks/`。

### 3. 在 WSL 里桥接 nmem（按需）

如果 Copilot CLI 跑在 WSL 里，而 `nmem` 是通过 Windows 上的 Mem 桌面应用提供的，就在 WSL 中创建下面这个桥接脚本：

```bash
mkdir -p ~/.local/bin && cat > ~/.local/bin/nmem << 'SHIMEOF'
#!/bin/bash
python3 - "$@" <<'PY'
import subprocess
import sys
cmd = subprocess.list2cmdline(["nmem.cmd", *sys.argv[1:]])
raise SystemExit(subprocess.run(["cmd.exe", "/s", "/c", cmd]).returncode)
PY
SHIMEOF
chmod +x ~/.local/bin/nmem
```

这样 WSL 里的 Copilot 也能干净地调用 Windows 侧的 `nmem`。

### 4. 想让 Copilot 更主动时，加上项目引导

插件会自己处理 Working Memory 启动和会话捕获。如果你还希望 Copilot 在这种经常续接历史任务的仓库里更主动地搜索或蒸馏，把插件里的 `AGENTS.md` 合并到项目根目录：

```bash
git clone https://github.com/nowledge-co/community.git /tmp/nowledge-community
cp /tmp/nowledge-community/nowledge-mem-copilot-cli-plugin/AGENTS.md ./AGENTS.md
rm -rf /tmp/nowledge-community
```

如果仓库里已经有 `AGENTS.md`，请合并 Nowledge 相关部分，而不是直接覆盖。

### 5. 需要远程 Mem 时配置

```bash
nmem config client set url https://mem.example.com
nmem config client set api-key nmem_your_key
```

这会写入插件和 `nmem` 共用的本地客户端配置。

## 自动发生的事

插件会通过 Copilot CLI hooks 做四件事：

- **SessionStart** 在 startup、resume、clear 时：用 `nmem --json wm read` 读取 Working Memory
- **SessionStart** 在 compaction 后：重新读取 Working Memory，并提醒 Copilot 保存重要进展
- **UserPromptSubmit**：把搜索与保存提示放到每一轮附近
- **Stop**：在每次回复结束后，异步捕获当前 Copilot 会话

Stop hook 只会追加新的对话内容，不会每次都把整段历史重新导入。

## Skills

Copilot 这条集成刻意保持了一个更简单的用户表面。它不再额外附带一层独立的命令文档，而是依赖 skills 和底层的 `nmem` CLI。

| Skill | 什么时候有用 | 它做什么 |
|-------|-------------|----------|
| read-working-memory | 会话开始，"我最近在做什么？" | 读取当天的 Working Memory 简报 |
| search-memory | 需要过去工作、过去理由、过去讨论时 | 搜索持久记忆和历史线程 |
| distill-memory | 出现了真正值得留下的决策或经验 | 把长期知识保存进 Mem |
| save-thread | "Save this session""Checkpoint this" | 在明确请求时保存一条简洁的会话摘要线程 |

如果你只是想直接排查连接问题，可以在终端里运行 `nmem status`。

## 更新

```bash
copilot plugin marketplace update nowledge-community
copilot plugin update nowledge-mem
```

然后重启 Copilot CLI。

## 安全地自定义

优先使用 Copilot 自己的 instruction files，而不是去改已安装插件：

- 仓库共享规则放 `.github/instructions/*.instructions.md`
- 跨仓库的个人规则放 `~/.copilot/instructions/*.instructions.md`

完整对照表见[安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## 常见问题

**找不到 nmem**

用 `pip` 或 `pipx` 安装 `nmem-cli`，或者直接使用 Mem 桌面应用附带的 CLI。

**看不到 Working Memory**

安装后先重启 Copilot CLI。然后运行 `nmem status`，确认本地客户端确实能连到 Mem。

**线程没有出现在 Mem 里**

安装或更新后先重启 Copilot CLI，再检查 `~/.copilot/nowledge-mem-hooks/hook-log.jsonl`。如果你用的是旧版安装方式，或者正在本地开发插件，也仍然可以从源码目录手动运行 `scripts/install-hooks.sh` 作为兼容回退。

**Copilot 只会读取 Working Memory，不太会继续搜索或蒸馏**

这通常说明启动钩子已经正常工作，但仓库里给 Copilot 的行为引导还不够强。把插件里的 `AGENTS.md` 合并到项目根目录，再用 continuation 型问题重试。

## 相关内容

- [集成概览](https://mem.nowledge.co/zh/docs/integrations)
- [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli)
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)
- [远程访问](https://mem.nowledge.co/zh/docs/remote-access)

---

# Gemini CLI

通过官方扩展路径安装 Nowledge Mem，并在大约一分钟内让 Gemini CLI 连上本地或远程 Mem。

## 现已上线 Extensions Gallery

最顺手的方式是直接在官方 Gemini Extensions Gallery 安装 **Nowledge Mem**，重启 Gemini CLI，把底层的记忆操作交给 `nmem`。

如果你更喜欢手动安装，页面底部依然保留了 GitHub 和本地目录安装方式，但对大多数用户来说，上面的官方扩展路径已经是默认推荐。

对大多数用户来说，心智模型可以很简单：先从官方 Gallery 安装扩展，确认一次 `nmem` 正常，再开一个新的 Gemini 会话。

## 开始之前

- Nowledge Mem 已在本地运行（[安装指南](https://mem.nowledge.co/zh/docs/installation)），或你已经有可访问的远程 Mem 服务
- 已安装 Gemini CLI
- `nmem` 在你的 `PATH` 中

如果你已经在同一台机器上运行 Nowledge Mem 桌面应用，最省事的方式是打开 Settings -> Preferences -> Developer Tools -> Install CLI。这样会把随应用附带的 `nmem` 安装到 PATH，并直接连上默认的本地 Mem 服务。

你也可以单独安装 `nmem`：

```bash
# 方式一：pip
pip install nmem-cli

# 方式二：uvx
curl -LsSf https://astral.sh/uv/install.sh | sh
uvx --from nmem-cli nmem --version
```

## 一分钟安装

### 1. 在扩展库中找到 Nowledge Mem

打开 [Gemini CLI Extensions Gallery](https://gemini-cli.gallery/)，搜索 **Nowledge Mem**。

### 2. 安装并重启

安装扩展，然后重启 Gemini CLI。

### 3. 运行一次 nmem status

对于同机默认配置，它应该指向 `http://127.0.0.1:14242 (default)`。

### 4. 打开新的 Gemini CLI 会话

扩展会自动加载 `GEMINI.md`、发现命令，并让技能在需要时可用。

## 更新

```bash
gemini extensions update nowledge-mem-gemini-cli
```

如果你更习惯通过扩展库更新，也可以直接在 Gemini CLI 扩展库里更新 Nowledge Mem。更新后记得重新开启一个新的 Gemini 会话。

## 安全地自定义

想调整行为时，优先使用 Gemini 自己的上下文文件：

- 项目里的 `GEMINI.md`
- 如果你想做个人默认规则，可用 `~/.gemini/GEMINI.md`
- 如果规则变长，可以继续用 `@file.md` 拆分

不要直接去改已安装扩展里的文件。完整对照表见[安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## 第一次成功应该看到什么

当下面几件事成立时，就说明 Gemini 已经接通：

- 你已经从 Gallery 成功安装扩展
- `nmem status` 可以正常返回
- 新开的 Gemini 会话里可以看到 Nowledge 命令
- `save-thread` 导入的是真实 Gemini 会话，而 `save-handoff` 仍然只是单独的摘要路径

## 远程 Mem

推荐的远程配置方式是：

```bash
nmem config client set url https://mem.example.com
nmem config client set api-key nmem_your_key
```

这会写入这台机器上的共享客户端配置。`nmem t save --from gemini-cli` 会在运行 Gemini 的那台机器上读取本地 Gemini 会话文件，再把规范化后的线程消息上传到 Mem。远程 Mem 服务器不需要直接访问 `~/.gemini`。

## 你会得到什么

- Gemini 原生的上下文注入、生命周期钩子、命令和技能
- 通过 `nmem t save --from gemini-cli` 捕获完整会话
- 当内置命令不够时，仍然可以直接调用 `nmem`
- 本地与远程共用一套清晰的 `nmem` 认证方式

## 命令

- `/nowledge:read-working-memory`
- `/nowledge:search-memory <query>`
- `/nowledge:distill-memory`
- `/nowledge:save-thread`
- `/nowledge:save-handoff`
- `/nowledge:status`

## 真实会话保存 与 交接摘要

Gemini 有两条不同的保存路径，而且应该继续严格区分：

- `save-thread` 会通过 `nmem t save --from gemini-cli` 导入 Gemini 的真实会话记录
- `save-handoff` 会保存一个适合中断后继续的交接摘要

## 手动安装

如果你更喜欢从 GitHub 或本地目录安装，可以使用下面这条路径：

```bash
git clone https://github.com/nowledge-co/nowledge-mem-gemini-cli.git
cd nowledge-mem-gemini-cli
gemini extensions link .
```

Gemini 官方文档也支持 `gemini extensions install <source>`（GitHub 或本地 source），以及 `gemini extensions link <path>`（本地开发）。对大多数用户来说，上面的已上架官方扩展安装流程仍然是最简单的选择。

## 相关内容

- [集成总览](https://mem.nowledge.co/zh/docs/integrations)
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)
- [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli)
- [远程访问](https://mem.nowledge.co/zh/docs/remote-access)
- [Copilot CLI](https://mem.nowledge.co/zh/docs/integrations/copilot-cli)
- [Alma](https://mem.nowledge.co/zh/docs/integrations/alma)

---

# Alma

Alma 专属 Nowledge Mem 插件。12 个工具、自动回忆、Access Anywhere 远程支持、设置即时生效。

记忆随 Alma 对话流动。从插件市场一键安装，无需额外配置。通过 Access Anywhere 连接远程 Mem 实例，跨设备使用。

来源：[community/nowledge-mem-alma-plugin](https://github.com/nowledge-co/community/tree/main/nowledge-mem-alma-plugin)

## 第一次成功应该是什么样

安装插件后，在一个新的 Alma 线程里发一条消息，然后确认 `nowledge_mem_status` 可用，并且插件已经能从 Mem 里做回忆，这就说明这条路径已经接通了。

## 安装步骤

### 1. 从 Alma 插件市场安装

在 Alma 中打开 **设置** → **插件** → **市场**，搜索 **Nowledge Mem**，点击 **安装**。

### 2. 如需请重启 Alma

## 更新

在 Alma 中打开 **设置** → **插件** → **市场**，找到 Nowledge Mem，如有新版本点击 **更新** 即可。

## 安全地自定义

Alma 这条集成没有单独的插件级 override 文件。想做稳定的行为调整：

- 优先使用 Alma 的插件设置，比如远程模式、回忆策略、对话捕获和 space
- 如果你想让智能体的表达方式或保存偏好不同，优先走 Alma 自己的 prompt / instruction 入口，而不是改插件代码
- 不要为了一个小偏好去修改安装后的插件文件

完整对照表见[安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## 插件功能

| 功能 | 说明 |
|------|------|
| 自动回忆 | 每个线程的第一条外发消息发送前，自动注入工作记忆简报与相关记忆 |
| 12 个工具 | 记忆：query、search、store、show、update、delete。线程：search、show、create、delete。另有工作记忆简报和状态诊断 |
| 随处访问 | 在插件设置中配置 API URL + API Key，连接远程 Mem 实例 |
| 设置即时生效 | 修改 API URL、API Key、回忆策略或自动捕获后立即生效，无需重启 |
| 状态诊断 | `nowledge_mem_status` 显示连接模式、服务器状态、CLI 可用性和当前配置 |
| 实时对话同步 | 空闲几秒后、切换线程、退出应用时自动保存对话（默认开启） |
| 本地优先 | 使用 `nmem` CLI，不启用 Access Anywhere 则无需联网 |

## 对话保存

对话在日常使用中自动同步到 Nowledge Mem——空闲几秒后、切换线程时、退出应用时都会触发保存，无需任何操作。保存的对话会出现在桌面端的"对话"页面，之后可以提炼为结构化记忆。可在插件设置中通过 `autoCapture` 关闭。

对话过程中，AI 也可能主动使用 `nowledge_mem_store` 工具将有价值的决策、结论或偏好保存为记忆。这是 AI 根据对话内容自主判断的——只有真正值得长期保留的信息才会被保存，日常闲聊不会被记录。

如果你想让 AI 保存某条信息，直接说"把这个存到记忆里"就行。

## 全库备份（可选）

若你需要**全部 Alma 对话的可携带归档**（迁入新资料库、留存备查或更换设备），在 Alma 中使用 **设置 → 数据 → 导出全部对话**，会生成包含 `threads.json` 的 `alma-backup-*.zip`。

在 Nowledge Mem 中打开 **对话 → 导入 → 批量导入** 并选择该 ZIP。导入的对话来源标记为 `alma`，与插件实时同步一致，筛选与来源展示保持一致。其他批量格式与命令行说明见[导入已有对话](https://mem.nowledge.co/zh/docs/import-existing-conversations)与[对话 — 批量导入](https://mem.nowledge.co/zh/docs/threads#bulk-import)。

## 随处访问

连接远程 Nowledge Mem 实例：

1. 在 Alma 中打开 **设置** → **插件** → **Nowledge Mem**
2. 设置 **API URL** 为远程服务器地址（如 `https://mem.example.com`）
3. 设置 **API Key** 为你的 `nmem_...` 密钥
4. 修改立即生效。使用 `nowledge_mem_status` 验证连接

API Key 仅通过环境变量传递，不会被记录到日志。

## Spaces

Alma 适合按 profile 保持一个稳定的记忆 lane。

- 如果一个 Alma profile 本来就对应一个固定上下文，可以直接设置 `nowledgeMem.space`
- 如果你的启动器已经能提供可靠的 lane 变量，可以使用 `nowledgeMem.spaceTemplate`
- 如果这个 Alma 实例始终只服务一个 space，也可以直接用 `NMEM_SPACE="Research Agent"` 启动

如果 Alma 本身并不知道真实的 Agent 身份，就不要硬做"每个 Agent 自动分 space"的路由。此时更稳妥的做法是：一个 Alma profile 对应一个 space，或者继续留在 `Default`。

## 相关

- [集成概览](https://mem.nowledge.co/zh/docs/integrations)：原生集成、复用包、MCP 与浏览器捕获
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code) · [Claude Desktop](https://mem.nowledge.co/zh/docs/integrations/claude-desktop) · [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli) · [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw) · [Raycast](https://mem.nowledge.co/zh/docs/integrations/raycast) · [内置 Web 聊天](https://mem.nowledge.co/zh/docs/integrations/built-in-web-chats) · [Gemini CLI](https://mem.nowledge.co/zh/docs/integrations/gemini-cli) · [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw)

---

# OpenClaw × Nowledge Mem

5 分钟配置指南，让 OpenClaw 同时拥有无损会话记忆和跨工具共享记忆。

## 一行安装

```bash
openclaw plugins install clawhub:@nowledge/openclaw-nowledge-mem
```

配置好之后：你在 OpenClaw 里聊出来的内容会进 Mem，能搜得到；定时跑的那条线默认不会挤进「对话」列表。该留的重要时刻仍可提炼成带 `sourceThreadId` 的记忆，你在别的工具里存下来的知识也一样能被读到。

此外，Mem 不只是把文字堆在一起。相关知识会连成图谱，观点怎么变会有迹可循；开了后台处理后，工作记忆简报、冲突提示、多来源综合出的 crystals 也会回流到 OpenClaw。

## 第一次成功应该是什么样

最快的验证方式很简单：先记住一条事实，在新会话里把它问回来，再确认这段会话本身也已经成为可搜索的线程。

### 已经发布到 ClawHub

这个包现在已经发布在 ClawHub。想明确指定来源时，直接使用 `clawhub:` 前缀即可。若省略前缀，OpenClaw 也会优先从 ClawHub 解析，再回退到 npm。

## 让 AI 帮你完成配置

如果你想让 OpenClaw 或另一个 AI 代理帮你完成安装和配置，把下面这段话直接交给它：

**AI 代配置**

```
Read https://nowled.ge/openclaw-skill and follow it to install, configure, verify, and explain Nowledge Mem for OpenClaw.
```

这份指南是给 AI 代理读的，不是给人逐行照着操作的。它会处理本地模式、远程模式、可选 API 认证、显式信任设置、重启、验证，以及后续建议。

## 开始之前

需要准备：

- **Nowledge Mem** 已在本地运行（[安装](https://mem.nowledge.co/zh/docs/installation)）
- **OpenClaw 2026.4.5** 或更高版本（[OpenClaw 入门](https://openclaw.ai/)）
- `nmem` CLI 在你的 PATH 中。在 Nowledge Mem 中，打开 **设置 > 偏好设置 > 开发者工具**，点击 **安装 CLI**。或独立安装：`pip install nmem-cli`

```bash
nmem status        # 应显示 Nowledge Mem 正在运行
openclaw --version
```

## 设置

### 1. 安装插件

```bash
openclaw plugins install clawhub:@nowledge/openclaw-nowledge-mem
```

### 安全地自定义

**1. 可选但推荐：把非内置插件加入信任 allowlist**

如果 OpenClaw 提示 `plugins.allow` 为空，可以加入：

```json
{
  "plugins": {
    "allow": [
      "openclaw-nowledge-mem"
    ]
  }
}
```

如果你还用了 `plugins.load.paths` 或 `openclaw plugins install --link` 的本地副本，也要一起检查。OpenClaw 的 allowlist 按插件 id 生效，不会固定来源路径。

**2. 重启 OpenClaw 并验证**

```bash
openclaw
nowledge-mem status
```

看到 Nowledge Mem 可访问即配置成功。

如果你不是通过 `openclaw plugins install` 安装，而是手动维护配置，请确认 `plugins.slots.memory` 是 `openclaw-nowledge-mem`，并且 `plugins.entries.openclaw-nowledge-mem.enabled` 为 `true`。

本地模式不需要 API key。如果你要连接远程 Nowledge Mem 服务器，请设置 `apiUrl`；如果那台服务器开启了认证，再额外设置 `apiKey`。

## Spaces

OpenClaw 比一般单 Agent 工具更适合做 lane 映射，但前提是宿主真的知道当前是谁在运行。

- 如果一个 OpenClaw profile 或进程本来就属于一个固定 lane，直接设置 `space`
- 如果你的启动器已经提供可信的身份变量，再使用 `spaceTemplate`
- 如果没有可靠的身份信号，就不要硬做自动路由。一个 profile 对应一个 space，往往比"看起来聪明"的猜测更稳
- 如果只是单 lane 使用，也可以直接用 `NMEM_SPACE="Research Agent"` 启动当前 OpenClaw 进程。

## 验证配置（1 分钟）

在 OpenClaw 聊天中依次执行：

1. `/remember 我们为任务事件选择了 PostgreSQL`
2. `/recall PostgreSQL` - 应立即找到
3. `/new` - 开启新会话
4. 问：`任务事件的数据库我们选的什么？` - 跨会话记住了
5. 问：`这周我都做了什么？` - 按周浏览
6. 问：`2月17日我在忙什么？` - 精确到某一天
7. `/forget PostgreSQL 任务事件` - 删除干净

如果以上七步都顺利，记忆系统已完整运作。

## 你能做什么

### 对话留下，定时任务不塞进来

你在窗口里亲自聊的内容会落成线程，以后搜得到。像 `cron-worker` 这种自动化会话，插件会直接跳过，不和你的手谈挤在同一张列表里。真有要留的东西，再提炼成结构化记忆，用 `sourceThreadId` 一键回到原文。

### 用图谱记忆，而不是平铺的存档

每条记忆都可以连到相关实体、同一主题更早或更新的版本，以及它来自的源对话。这样 OpenClaw 做的就不只是关键词检索，而是能追踪一个决策怎么变化、它和哪些主题相连、答案来自哪里。

### 让知识在后台继续成长

当你在 Nowledge Mem 里开启 Background Intelligence 后，系统会在会话结束后继续工作：去重重叠内容、找出矛盾、生成 Working Memory 简报，并在多条记忆逐渐收敛时形成知识结晶（crystals）。下次你使用 OpenClaw 时，这些结果就已经在那里了。

### 记住任何事情

告诉 AI `/remember 我们决定不用微服务，原因是团队太小`，下周换一个会话，直接问"微服务那个决定是怎么说的"，它能找到。

## 工作原理

### 每轮对话的工作流

每次你发送消息，插件会在 AI 处理之前注入行为引导。AI 随后决定调用哪些工具。

行为技能和始终在线的引导提示 AI **回答前先搜索**、**做完决定后保存**。每个工具的触发时机：

| 场景 | 工具 | 做什么 |
|------|------|--------|
| 用户提问 | `memory_search` | 回答前搜索知识库，返回 `sourceThreadId` |
| 做了决策、学到新东西 | `nowledge_mem_save` | 结构化保存：类型 + 标签 + 时间 |
| "上周我在做什么？" | `nowledge_mem_timeline` | 按天分组的活动流，支持精确日期 |
| "X 和 Y 有什么关联？" | `nowledge_mem_connections` | 图谱遍历：边、实体、演化链、来源 |
| 需要今日重点/优先事项 | `nowledge_mem_context` | 读取工作记忆每日简报 |
| 记忆有 `sourceThreadId` | `nowledge_mem_thread_fetch` | 获取完整来源对话，支持分页 |
| "找一下我们讨论 X 的对话" | `nowledge_mem_thread_search` | 按关键词搜索过去的对话 |
| "忘掉 X" | `nowledge_mem_forget` | 按 ID 或搜索词删除 |
| "我的配置对吗？" | `nowledge_mem_status` | 显示配置、连接状态和版本 |

斜杠命令快捷方式：`/remember`、`/recall`、`/forget`

### 会话生命周期（自动捕获）

会话结束时，对话自动被捕获并可选地提炼为结构化记忆。无需用户操作。

你在 OpenClaw 里正常聊出来的会话，会像其他已连接的助手一样自动进 Mem，搜得到。

定时任务、cron 那类隔离运行默认不同步。诊断里看到 `cron-worker` 不稀奇；对话列表刻意不收它们，是产品上的选择，不是漏了。

提炼跟着「对话真的变长了」走：`agent_end`，或者开了上下文引擎时的普通轮次。只有压缩检查点的那一瞬，不会单独再开一轮提炼。

如果你开启了上下文引擎，提示词组装会交给它；但线程捕获仍保留生命周期钩子做兜底。这样更稳，不会因为某一条运行路径漏掉一次回调，就把整段会话丢掉。

提炼结果带 `sourceThreadId`，要回原文一戳就行。

### 渐进式检索（记忆 -> 线程 -> 消息）

从对话中提炼的记忆携带 `sourceThreadId`，形成检索链：搜索记忆 -> 追溯来源对话 -> 分页读取完整消息。

两个进入过去对话的入口：

- **从记忆出发**: `memory_search` 或 `memory_get` 返回 `sourceThreadId`，然后获取来源对话
- **直接搜索**: `nowledge_mem_thread_search` 按关键词查找对话，然后获取任意结果

### 三种模式

插件支持三种运行模式。根据你想要多少保障、愿意花多少 token 预算来选择。

| 模式 | 配置 | 行为 | Token 开销 |
|------|------|------|------------|
| **默认**（推荐） | `sessionContext: false` | AI 按需调用 10 个工具。会话结束时自动捕获 + 提炼。 | 开销最低，由 AI 自己判断何时搜索 |
| **会话上下文** | `sessionContext: true` | 每次提示时自动注入工作记忆和相关记忆，同时仍可使用全部 10 个工具。 | 每轮提示更大，但从第一轮开始就带着上下文 |
| **最小模式** | `sessionDigest: false` | 仅工具调用，不自动捕获。 | 只保留那条简短系统提示的开销 |

#### 选哪个模式？

- **大多数用户**：从默认模式开始。AI 每轮收到行为引导，提示它回答前先搜索、做完决定后保存。大多数对话场景下效果良好。
- **短会话或需要高准确性**：开启 `sessionContext`。这确保从第一轮开始就有相关记忆在上下文中，AI 不需要自行判断是否搜索。代价是每轮提示会更大。
- **完全手动控制**：设置 `sessionDigest: false`。你自己决定什么该保存（通过 `/remember` 或 `nowledge_mem_save`），不会自动捕获任何内容。

### sessionContext - 会话上下文注入

开启后，插件在每次提示时自动注入上下文：

1. 读取**工作记忆**，即后台智能每天早上生成的今日简报
2. 根据当前消息在知识图谱中**搜索相关记忆**
3. 将检索到的内容作为这次回答的上下文插入，同时把稳定的行为引导放在系统提示区域

开启 `sessionContext` 后，行为引导会自动调整，告诉 AI 上下文已经注入，`memory_search` 仅用于特定的后续查询，而非广泛的回忆。这样可以避免重复搜索相同的上下文。

适用于短会话和关键工作流，确保从第一轮开始就有完整的记忆上下文。

### sessionDigest - 对话线程 + LLM 智能提炼（默认开启）

在会话生命周期事件（`agent_end`、`after_compaction`、`before_reset`）时执行两步操作：

1. **对话线程保存**（你亲自聊的才算）。
   整段对话会追加进 Mem 的持久线程，用 `nowledge_mem_thread_search` 能搜到。OpenClaw 给定时、cron 那类隔离运行发了另一套会话键，插件碰到就跳过，免得后台任务和你的手谈叠在同一条时间线上。

2. **LLM 智能提炼**（有价值时才执行）。
   保存线程后，先用一次轻量级 LLM 筛选判断对话中是否有值得保存的内容（决策、洞察、偏好、事实）。如果有，执行完整的提炼流程，提取带有正确类型、标签和时间信息的结构化记忆。

支持**任何语言**。

- **上下文压缩**：当 OpenClaw 压缩长对话时，插件会先捕获对话记录，不会丢失任何内容。
- **消息去重**：线程追加按消息 ID 幂等，不会出现重复内容。

## 常见问题

**找不到 nmem 命令**

```bash
pip install nmem-cli
nmem status
curl -sS http://127.0.0.1:14242/health
```

**记忆工具能用，但这段会话本身没有出现在 Threads 里**

在正常聊天会话里运行 `nowledge_mem_status`，重点确认：

- `sessionDigest` 仍然开启
- 后端可达
- 当前走的是哪条捕获路径
- 最近有没有改过插件设置但还没重启 OpenClaw

如果你启用了 `plugins.slots.contextEngine: "nowledge-mem"`，那么插件 0.8.6+ 会把生命周期钩子保留下来，作为线程同步的兜底。在当前一些 OpenClaw 安装器版本里，这个值也可能被自动写成 `openclaw-nowledge-mem`；插件 0.8.18+ 会把它当作同一个引擎处理。若你还在 0.8.5 或更早版本，一个很有效的隔离方法是先临时移除 `contextEngine` slot，重启 OpenClaw，再看 hook-only 路径下线程捕获是否恢复。

要注意：OpenClaw 的插件设置是在重启后才真正生效的。如果你之前把 `sessionDigest` 关掉了，但当时还没重启，线程同步可能会暂时看起来还在继续；等到下一次重启后，才会真正停下来。

健康的 OpenClaw 线程同步，表现应该是这样：

- 一段你看得见的当前聊天，只对应 Mem 里的一条线程
- 运行 `/new` 或 `/reset` 后，会开始一条新的 Mem 线程
- compaction 不会把同一段聊天拆出第二条线程
- `temp:slug-generator` 这种辅助会话不会出现在列表里
- `/new` 或 `/reset` 的启动提示，不会被保存成第一条用户消息

想直接看最近同步进来的 OpenClaw 线程，可以运行：

```bash
nmem t list --source openclaw -n 20
```

**只有 `memory_search` 和 `memory_get` 能用——保存的内容存到了本地文件**

这通常是因为记忆插槽仍然指向 OpenClaw 内置的 `memory-core`，而不是 Nowledge Mem。OpenClaw 3.22 起，如果配置中没有显式指定记忆插槽，会默认使用 `memory-core`。如果你是手动安装插件、或在升级过程中配置被重置过，插槽可能需要重新设置。

确认配置中有显式的插槽声明：

```json
{
  "plugins": {
    "slots": {
      "memory": "openclaw-nowledge-mem"
    }
  }
}
```

或者重新安装，安装命令会自动设置插槽：

```bash
openclaw plugins install clawhub:@nowledge/openclaw-nowledge-mem
```

改完后重启 OpenClaw。

**插件工具不可用**

插件工具在插件被允许加载后会自动注册。确保插件在 `plugins.allow` 中：

```json
{
  "plugins": {
    "allow": [
      "openclaw-nowledge-mem"
    ]
  }
}
```

不要在 `tools.allow` 中填入 `nowledge_mem_*` 工具名——OpenClaw 会静默丢弃仅含插件工具的允许列表。无需任何 `tools.*` 配置。

**多个代理并发运行时搜索变慢**

同时运行很多代理（10 个以上）时，搜索性能可能下降，因为所有操作共用一条数据库连接。建议：

- 升级到 Nowledge Mem v0.6.12+（后端）——搜索响应不再被计分写入阻塞
- 如果代理走的是 CLI 通道，可以在插件配置中设置远程 API 地址，以减少子进程开销

**搜索只找到一两条结果**

把 `maxContextResults` 调高到 `8` 或 `12`。

## 配置

正常通过 npm 安装时，开箱即用。安装器已经帮你启用了插件并选好了 memory slot。

修改设置：打开 OpenClaw 控制面板，进入 **Automation > Plugins**。在 **Plugin Entries** 下展开 **Nowledge Mem**，再展开 **Nowledge Mem Config**。也可以在搜索栏输入"nowledge"直接定位。修改后重启 OpenClaw 生效。

| 设置 | 默认值 | 说明 |
|------|--------|------|
| Session context injection | 关 | 每次提示时注入工作记忆和相关记忆 |
| Session digest at end | 开 | 会话结束时捕获对话 + 提炼关键记忆 |
| Minimum digest interval | 300s | 会话提炼最短间隔秒数（0 = 无限制） |
| Max context results | 5 | 注入的记忆数量（1-20） |
| Min recall score | 0 | 仅注入相关性高于此阈值的记忆（0-100%），0 表示全部包含 |
| Max thread message chars | 800 | 每条捕获线程消息保留的最大字符数（200-20000），长代码或技术对话可适当调高 |
| Corpus supplement | 关 | 将你的知识接入 OpenClaw 的 dreaming 系统（详见下方） |
| Corpus max results | 5 | 每次 dreaming 搜索返回的最大结果数（1-20） |
| Corpus min score | 0 | dreaming 结果的最低分数（0-100%），0 表示全部包含 |
| Dreaming | 由 OpenClaw 管理 | 可选的 OpenClaw dreaming 设置。当 Nowledge Mem 占用 memory slot 时，OpenClaw 可能会把原生 `dreaming` 对象写在这里。真正运行 dreaming 引擎的仍然是 `memory-core`。 |
| Server URL | 空 | 远程服务器 URL（留空 = 本地） |
| API key | 空 | 远程模式 API 密钥 |

## 远程访问

连接另一台机器上的 Nowledge Mem 服务器：先在这台机器上执行一次：

```bash
nmem config client set url https://<your-url>
nmem config client set api-key nmem_...
```

这会写入这台机器上的共享客户端配置，`nmem`、OpenClaw、Bub、Claude Code 等集成都会复用它。也可以在 OpenClaw 仪表盘的插件设置中填写 `Server URL` 和 `API key`。插件内部无论走哪条路径，都会复用同一组解析后的凭据：基于 CLI 的记忆工具和基于 API 的线程同步都会使用同一个 `apiUrl` 与 `apiKey`。API 密钥不会出现在日志或命令行历史中。详见：[随处访问 Mem](https://mem.nowledge.co/zh/docs/remote-access)。

## 遇到问题？

参见上方"常见问题"部分。

## 为什么用 Nowledge Mem 而不是其他方案？

Nowledge Mem 提供跨工具共享记忆、图谱关系追踪、后台智能处理等能力，让 OpenClaw 不只是存取孤立文本，而是在一个完整的知识生态中运作。

## 与 memory-core 协同（v0.8.0+）

Nowledge Mem 可以作为 OpenClaw 的 memory slot 插件，完整替代 `memory-core`；也可以与 `memory-core` 协同工作，通过 corpus supplement 模式将跨工具知识接入 `memory-core` 的管线。

### corpus supplement 做了什么

当 memory-core 执行搜索时，它也会同时查询 Nowledge Mem。你的知识图谱中的结果和 memory-core 自身的结果一起排序和评分：

- memory-core 的召回会包含你的跨工具知识
- Dreaming（实验性）可以把高频召回的 Nowledge Mem 内容提升到 `MEMORY.md`
- 每周模式提取可以发现知识图谱与 memory-core 数据之间的关联

### 配置方式

保持 memory slot 为 memory-core（或不设置）。在插件设置中打开 `corpusSupplement: true`，或通过环境变量 `NMEM_CORPUS_SUPPLEMENT=true`。

插件会自动处理去重。当 Nowledge Mem 是 memory slot 时，它直接处理召回；当 memory-core 是 memory slot 且 corpus supplement 开启时，它接入 memory-core 的管线。同一条内容不会出现两次。

如果你在 Nowledge Mem 这个插件条目下看到顶层的 `dreaming` 对象，也不用担心，这属于正常现象。新版 OpenClaw 会把 dreaming 设置写到当前选中的 memory-slot 插件上。Nowledge Mem 只负责接受这份配置，不会自己实现 dreaming 引擎；真正运行 dreaming 的仍然是 memory-core。

### 怎么选

| 配置 | 适合场景 |
|------|----------|
| Nowledge Mem 作为 memory slot（默认） | 完整的 10 个工具、结构化记忆类型、对话溯源。一个系统管理一切。 |
| memory-core + corpus supplement | 你在用 memory-core 的 `MEMORY.md` 工作流，或者想用 dreaming。跨工具知识仍然会接入。 |

两种配置下，对话捕获和蒸馏的效果是一样的。区别在于由哪个系统负责召回和长期整合。

## 上下文引擎（v0.7.0+）

从插件 v0.7.0 起，Nowledge Mem 可以作为 OpenClaw 的完整**上下文引擎**运行，而不仅仅是提示钩子。这让它能更深入地参与 OpenClaw 的生命周期：

- **记忆感知的上下文压缩**：当 OpenClaw 压缩长对话时，你知识图谱中已保存的关键决策和经验会以引用方式保留，不会在摘要过程中丢失
- **子代理记忆继承**：当 OpenClaw 启动并行研究代理时，它们会自动继承你的记忆上下文
- **会话预热**：工作记忆在第一轮对话前就加载好，上下文从一开始就已就绪
- **逐轮捕获**：对话在每轮结束后都会被捕获，而不只在会话结束时

激活方式——在 OpenClaw 配置中添加：

```json
{
  "plugins": {
    "slots": {
      "memory": "openclaw-nowledge-mem",
      "contextEngine": "nowledge-mem"
    }
  }
}
```

如果不激活上下文引擎，现有的钩子模式会继续正常工作。

如果激活了，上下文引擎负责提示词组装和逐轮捕获；生命周期钩子则继续留作线程同步的兜底路径。这样分工是刻意设计的：上下文注入不会重复，线程同步也不会把成败压在单一路径上。

## 给进阶用户

OpenClaw 的 `MEMORY.md` 工作区文件仍然有效，但记忆工具的实际调用全部由 Nowledge Mem 处理。两者可以共存。

插件内部使用同一套连接配置，但不强行走同一种传输方式。大多数记忆操作仍通过 `nmem` 完成；对话线程同步则直接请求 Mem 服务器，这样长会话会通过正常的请求体传输，而不是塞进一条很长的命令行参数。对用户来说不需要学两套配置：地址和密钥只配一次，本地模式和远程模式都按同样的方式工作。

## 相关

- [集成概览](https://mem.nowledge.co/zh/docs/integrations) - 原生集成、复用包、MCP 与浏览器捕获
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code) · [Claude Desktop](https://mem.nowledge.co/zh/docs/integrations/claude-desktop) · [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli) · [Alma](https://mem.nowledge.co/zh/docs/integrations/alma) · [Raycast](https://mem.nowledge.co/zh/docs/integrations/raycast) · [内置 Web 聊天](https://mem.nowledge.co/zh/docs/integrations/built-in-web-chats)

## 参考

- 插件源码：[nowledge-mem-openclaw-plugin](https://github.com/nowledge-co/community/tree/main/nowledge-mem-openclaw-plugin)
- OpenClaw 文档：[插件系统](https://docs.openclaw.ai/tools/plugin)
- 更新日志：[CHANGELOG.md](https://github.com/nowledge-co/community/blob/main/nowledge-mem-openclaw-plugin/CHANGELOG.md)

---

# Bub × Nowledge Mem

让 Bub 看到你在其他 AI 工具中积累的知识，也让 Bub 中的收获流回所有工具。

## 安装

```bash
pip install nowledge-mem-bub
```

Bub 通过 Tape 系统记录每次会话。Nowledge Mem 在此之上提供跨工具知识层：你在 Claude Code 中做的决策、在 Cursor 里设定的偏好、在 ChatGPT 中获得的洞察——都能在 Bub 里搜索到。反过来，你在 Bub 中学到的东西也会流回其他所有工具。

## 准备工作

- **Nowledge Mem** 正在本地运行（[安装指南](https://mem.nowledge.co/zh/docs/installation)）
- **Bub** 已安装（[bub.build](https://bub.build)）
- `nmem` CLI 在 PATH 中 — 在 Nowledge Mem 中前往 **设置 > 开发者工具 > 安装 CLI**，或 `pip install nmem-cli`

```bash
nmem status              # 确认 Nowledge Mem 正在运行
uv run bub --help        # 确认 Bub 可用
```

## 配置

### 1. 安装插件

```bash
pip install nowledge-mem-bub
```

### 2. 验证 hooks

```bash
uv run bub hooks
```

你应该在 `system_prompt`、`build_prompt` 和 `save_state` 下看到 `nowledge_mem`。

### 3. 试一个依赖知识的提问

问 Bub 一个需要过去知识的问题：

```bash
uv run bub run "我这周在做什么？"
```

如果 Nowledge Mem 中已有知识，智能体会通过 `mem.search` 找到它。这说明 Bub 已经能看到你其他工具里的知识了。

## 更新

```bash
python3 -m pip install --upgrade nowledge-mem-bub
```

## 安全地自定义

Bub 这条集成目前没有单独的持久化指引文件。想做稳定的行为调整：

- 优先使用环境变量，以及 Bub 自己的运行时 prompt / config
- 保持安装好的 Python 包为默认状态，不要为了语言、回忆风格或保存阈值去改 site-packages

完整对照表见[安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## 能做什么

### 从其他工具获取知识

问"我们之前关于数据库的决定是什么？"，智能体会搜索你在 Claude Code 中做的决策、ChatGPT 中的讨论、Cursor 中的笔记，不局限于当前 Bub 会话。

### 为所有工具保存知识

在 Bub 中得出结论时，`mem.save` 会将它保存下来，下次在 Claude Code、Cursor 或 ChatGPT 中都能找到。

### 带着今天的上下文开始

开启会话上下文模式后，Working Memory 和相关知识会在对话开始前就准备好，不用在不同工具间重复说明背景。

### 追溯想法的演变

`mem.connections` 展示一个决策如何随时间变化、在哪些工具中讨论过、有哪些源文档支撑。

## 两种模式

| 模式 | 配置 | 行为 |
|------|------|------|
| 默认 | 无需配置 | 智能体按需搜索和保存。对话自动流入 Mem，供其他工具发现。 |
| 会话上下文 | `NMEM_SESSION_CONTEXT=1` | 每轮自动注入 Working Memory 和相关知识。 |

建议从默认模式开始。如果你希望从第一句话就有完整的上下文回忆，再开启会话上下文。

## 工具

| 工具 | 说明 |
|------|------|
| `mem.search` | 搜索所有工具中的知识，支持标签和日期过滤。 |
| `mem.save` | 保存决策、洞察或偏好，让任何工具都能找到。 |
| `mem.context` | 读取今天的 Working Memory——关注领域、优先级、近期动态。 |
| `mem.connections` | 探索一条知识与其他知识的关联，跨工具、跨时间。 |
| `mem.timeline` | 按天分组的近期活动。 |
| `mem.forget` | 按 ID 删除一条记忆。 |
| `mem.threads` | 搜索所有工具中的历史对话。 |
| `mem.thread` | 获取完整对话消息，支持分页。 |
| `mem.status` | 连接状态和配置诊断。 |

所有工具也可作为 Bub 逗号命令使用：`,mem.search query=...`

## 环境变量

本地使用无需配置。

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `NMEM_SESSION_CONTEXT` | false | 每轮注入 Working Memory 和相关知识 |
| `NMEM_SESSION_DIGEST` | true | 将 Bub 对话流入 Mem，供其他工具发现 |
| `NMEM_API_URL` | （本地） | 远程 Nowledge Mem 服务器地址 |
| `NMEM_API_KEY` | （无） | 远程访问的 API 密钥 |

如果你想用一套持久化的共享配置，先在这台机器上执行 `nmem config client ...`。如果只是临时覆盖，环境变量仍然优先。

## Spaces

Bub 目前最稳妥的做法仍然是按进程分 lane。

- 如果一个 Bub 进程天然只属于一个上下文，直接设置 `NMEM_SPACE="Research Agent"`
- 如果你同时运行多个 Bub agent，就给每个进程各自设置一个 `NMEM_SPACE`
- 如果 Bub 这边没有可靠的 Agent 身份信号，就继续使用 `Default`

也就是说，Bub 现在适合"一个进程一个 lane"，而不是在同一个进程里伪造复杂的多 Agent 自动映射。

## 远程访问

```bash
nmem config client set url https://your-server
nmem config client set api-key your-key
```

详见[远程访问 Mem](https://mem.nowledge.co/zh/docs/remote-access)。

## 常见问题

**插件未加载** — 运行 `uv run bub hooks`，确认列表中有 `nowledge_mem`。确保 `nowledge-mem-bub` 与 Bub 安装在同一个 Python 环境中。

**nmem 未找到** — `pip install nmem-cli && nmem status`

**服务器无响应** — 启动 Nowledge Mem 桌面应用，或用 `nmem status` 查看诊断信息。

## 相关

- [集成概览](https://mem.nowledge.co/zh/docs/integrations)
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code) · [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw) · [Alma](https://mem.nowledge.co/zh/docs/integrations/alma) · [Gemini CLI](https://mem.nowledge.co/zh/docs/integrations/gemini-cli)
- 插件源码：[nowledge-mem-bub-plugin](https://github.com/nowledge-co/community/tree/main/nowledge-mem-bub-plugin)
- [tape.systems](https://tape.systems) · [bub.build](https://bub.build)

---

# OpenCode × Nowledge Mem

在 OpenCode 中使用跨工具知识库，让 OpenCode 的每次会话都能调用你在其他工具中积累的决策、经验和上下文。

```json
{
  "plugin": [
    "opencode-nowledge-mem"
  ]
}
```

OpenCode 是一款强大的终端编程智能体。Nowledge Mem 为它补充跨工具知识：来自 Claude Code、Cursor、Codex 等工具的决策和经验，在 OpenCode 中即刻可用。反过来，OpenCode 中产生的知识也会同步到你的其他工具。

## 准备工作

- **Nowledge Mem** 已在本地运行（[安装指南](https://mem.nowledge.co/zh/docs/installation)）
- **OpenCode** 已安装
- `nmem` CLI 已在 PATH 中。在 Nowledge Mem 中前往 **设置 > 开发者工具 > 安装 CLI**，或执行 `pip install nmem-cli`

```bash
nmem status         # 确认 Nowledge Mem 正在运行
opencode --version  # 确认 OpenCode 可用
```

## 安装

### 1. 添加插件

在 OpenCode 配置中添加插件：

**opencode.json**

```json
{
  "plugin": [
    "opencode-nowledge-mem"
  ]
}
```

如需全局生效：

**~/.config/opencode/opencode.json**

```json
{
  "plugin": [
    "opencode-nowledge-mem"
  ]
}
```

### 2. 重启 OpenCode

关闭并重新打开 OpenCode，让它加载新插件。

### 3. 验证集成

让 OpenCode 检查与 Nowledge Mem 的连接：

```
我最近在做什么？
```

你应该会看到 OpenCode 调用 `nowledge_mem_working_memory` 并返回你的当前上下文。看到这些就说明已经接通了：OpenCode 现在可以访问你在其他工具中积累的知识。

## 更新

固定特定版本：

```json
{
  "plugin": [
    "opencode-nowledge-mem@0.3.0"
  ]
}
```

## 安全地自定义

优先使用 OpenCode 自己的指引入口，而不是去改插件包文件：

- 项目级规则放在仓库里的 `AGENTS.md`
- 个人默认规则放在 `~/.config/opencode/AGENTS.md`
- 如果你更偏好宿主配置，也可以用 `opencode.json` 里的 `instructions`

不要直接修改已安装的 Nowledge Mem 插件包。完整对照表见[安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## 你能做什么

### 在 OpenCode 中查找其他工具的知识

问一句"数据库方案之前定了什么？"，OpenCode 会搜索你在 Claude Code 中做过的决策、ChatGPT 中获得的洞察、Cursor 中留下的笔记，而不仅仅是当前会话。

### 保存知识，全局可用

当你在 OpenCode 中得出结论，智能体会将其保存，你下一次打开 Claude Code、Cursor 或 ChatGPT 时都能找到。

### 会话一开始就有上下文

你的 Working Memory 简报和相关历史知识在你开口之前就已准备好，不用在不同工具间重复自己。

### 创建可恢复的交接

在 OpenCode 中收尾后，到 Claude Code 或任何其他工具中可以接着继续。决策、计划和上下文会自动带过去。

## 工具一览

| 工具 | 说明 |
|------|------|
| `nowledge_mem_working_memory` | 读取今日 Working Memory：关注领域、优先事项、近期动态。 |
| `nowledge_mem_search` | 跨工具搜索知识，支持标签、日期和深度模式过滤。 |
| `nowledge_mem_save` | 保存一个决策、洞察或偏好，让任何工具都能找到。 |
| `nowledge_mem_update` | 更新已有记忆的内容或元数据。 |
| `nowledge_mem_thread_search` | 搜索任何工具中的历史对话。 |
| `nowledge_mem_save_thread` | 保存当前会话的完整对话记录。 |
| `nowledge_mem_save_handoff` | 保存精简的交接摘要（由智能体撰写）。 |
| `nowledge_mem_status` | 连接状态与配置诊断。 |

## 会话捕获机制

Nowledge Mem 通过两种互补方式捕获 OpenCode 会话：

**后台自动同步。** 桌面应用会定期轮询 OpenCode 的会话数据库，根据你的同步策略导入对话。在 **设置 > 对话同步** 中启用 OpenCode 即可。自动同步不需要安装插件。

**插件完整会话捕获。** `nowledge_mem_save_thread` 通过 OpenCode SDK 读取当前会话的全部消息，以 HTTP 方式发送到 Nowledge Mem。幂等操作，支持大型会话，本地和远程模式均可使用。

**插件主动知识保存。** `nowledge_mem_save` 在对话中实时捕获决策和洞察，`nowledge_mem_save_handoff` 在收尾时创建精简摘要。两者都是对完整会话记录的高信号补充。

长时间会话中 OpenCode 可能会压缩上下文，插件的压缩恢复钩子会自动重新注入 Nowledge Mem 工具信息，确保智能体不会断开连接。

后台自动同步需要直接读取 OpenCode 的本地数据库，因此仅在 Nowledge Mem 和 OpenCode 运行在同一台机器上时可用。使用远程模式时，请通过插件的保存工具或在客户端执行 `nmem t save` 来同步。

## 配置

本地使用无需配置。

| 环境变量 | 默认值 | 说明 |
|----------|--------|------|
| `NMEM_API_URL` | (本地) | 远程 Nowledge Mem 服务器地址 |
| `NMEM_API_KEY` | (无) | 远程访问的 API 密钥 |

如果你想用一套持久化的共享配置，先在这台机器上执行 `nmem config client ...`。如果只是临时覆盖，环境变量的优先级仍然更高。

## 远程访问

```bash
nmem config client set url https://your-server
nmem config client set api-key your-key
```

参见[随处访问 Mem](https://mem.nowledge.co/zh/docs/remote-access)。

## 常见问题

**找不到 nmem 命令。** 执行 `pip install nmem-cli`，然后运行 `nmem status` 确认连接正常。

**服务器无响应。** 启动 Nowledge Mem 桌面应用，或用 `nmem status` 检查诊断信息。

**插件未加载。** 确认 `opencode.json` 的 `plugin` 数组中包含 `"opencode-nowledge-mem"`。修改配置后需要重启 OpenCode。

## 相关

- [集成概览](https://mem.nowledge.co/zh/docs/integrations)
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code) · [Pi](https://mem.nowledge.co/zh/docs/integrations/pi) · [Hermes Agent](https://mem.nowledge.co/zh/docs/integrations/hermes) · [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw) · [Alma](https://mem.nowledge.co/zh/docs/integrations/alma)
- 插件源码：[nowledge-mem-opencode-plugin](https://github.com/nowledge-co/community/tree/main/nowledge-mem-opencode-plugin)



---

# 第五部分：其他集成与使用场景

# Pi × Nowledge Mem

在 Pi 中使用跨工具知识库，让 Pi 的每次会话都能调用你在其他工具中积累的决策、经验和上下文。

## 一键安装

```bash
pi install npm:nowledge-mem-pi
```

Pi 是一款精简的终端编程智能体。Nowledge Mem 为它补充跨工具知识：来自 Claude Code、Cursor、Codex 等工具的决策和经验，在 Pi 中即刻可用。反过来，Pi 中产生的知识也会同步到你的其他工具。

## 准备工作

- **Nowledge Mem** 已在本地运行（[安装指南](https://mem.nowledge.co/zh/docs/installation)）
- **Pi** 已安装
- **nmem CLI** 已在 PATH 中。在 Nowledge Mem 中前往 `设置 > 开发者工具 > 安装 CLI`，或执行 `pip install nmem-cli`

```bash
nmem status  # 确认 Nowledge Mem 正在运行
pi --version # 确认 Pi 可用
```

## 安装

### 1. 安装插件包

```bash
pi install npm:nowledge-mem-pi
```

### 2. 验证集成

让 Pi 检查与 Nowledge Mem 的连接：

> Nowledge Mem 连上了吗？运行一下 status 技能。

你应该会看到连接信息和服务器可达的确认。看到这些就说明已经接通了：Pi 现在可以访问你在其他工具中积累的知识。

## 更新

```bash
pi update
```

## 安全地自定义

优先使用项目自己的 `AGENTS.md`，而不是去改安装后的包缓存。

- 如果你希望 Pi 在这个仓库里更主动地检索或保存，就把包里的行为指引合并进项目 `AGENTS.md`
- 把包文件保留为默认值，这样升级时不会把你的微调冲掉

Pi 目前没有额外独立的持久 override 文件，最稳妥的入口就是项目级行为指引。完整对照表见 [安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## 你能做什么

**在 Pi 中查找其他工具的知识**
问一句"数据库方案之前定了什么？"，Pi 就可以去搜索你在 Claude Code 中做过的决策、在 ChatGPT 中获得的洞察、在 Cursor 中留下的笔记，而不仅仅是当前会话。

**保存知识，全局可用**
当你在 Pi 中得出结论时，可以让 `distill-memory` 把它存下来。这样你下次打开 Claude Code、Cursor 或 ChatGPT 时都还能找到。

**会话一开始就有上下文**
把包里的行为指引合并进项目 `AGENTS.md` 后，Pi 就可以在会话开始时先读取当天的 Working Memory 和相关历史上下文，不用在不同工具间反复解释背景。

**创建可恢复的交接**
当你让 Pi 保存交接时，它会生成一份结构化摘要，方便你到 Claude Code 或其他工具里继续接着做。

## 技能一览

| 技能 | 说明 |
|------|------|
| read-working-memory | 读取今日 Working Memory：关注领域、优先事项、近期动态。 |
| search-memory | 跨工具搜索知识，支持标签和日期过滤。 |
| distill-memory | 保存一个决策、洞察或偏好，让任何工具都能找到。 |
| save-thread | 为当前会话生成一个结构化交接摘要。 |
| status | 连接状态与配置诊断。 |

## 配置

本地使用无需配置。

| 环境变量 | 默认值 | 说明 |
|----------|--------|------|
| NMEM_API_URL | (本地) | 远程 Nowledge Mem 服务器地址 |
| NMEM_API_KEY | (无) | 远程访问的 API 密钥 |

如果你想用一套持久化的共享配置，先在这台机器上执行 `nmem config client ...`。如果只是临时覆盖，环境变量的优先级仍然更高。

## 远程访问

```bash
nmem config client set url https://your-server
nmem config client set api-key your-key
```

参见 [随处访问 Mem](https://mem.nowledge.co/zh/docs/remote-access)。

## 常见问题

- **找不到 nmem 命令。** 执行 `pip install nmem-cli`，然后运行 `nmem status` 确认连接正常。
- **服务器无响应。** 启动 Nowledge Mem 桌面应用，或用 `nmem status` 检查诊断信息。
- **技能未加载。** 用 `pi list` 确认插件已安装。如果看不到 `nowledge-mem-pi`，请重新安装：`pi install npm:nowledge-mem-pi`。

## 相关

- [集成概览](https://mem.nowledge.co/zh/docs/integrations)
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)
- [OpenCode](https://mem.nowledge.co/zh/docs/integrations/opencode)
- [Hermes Agent](https://mem.nowledge.co/zh/docs/integrations/hermes)
- [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw)
- [Alma](https://mem.nowledge.co/zh/docs/integrations/alma)
- [Bub](https://mem.nowledge.co/zh/docs/integrations/bub)
- 插件源码：[nowledge-mem-pi-package](https://github.com/nowledge-co/community/tree/main/nowledge-mem-pi-package)

---

# Hermes Agent × Nowledge Mem

Hermes v0.7.0+ 原生记忆提供者。Working Memory 自动加载，相关知识会在每轮前浮现，并且会在会话结束时把清洗后的对话记录同步成 Mem 线程。

## 一键安装（插件模式）

```bash
bash <(curl -sL https://raw.githubusercontent.com/nowledge-co/community/main/nowledge-mem-hermes/setup.sh)
```

## 原生插件集成

Hermes v0.7.0+ 支持记忆提供者插件。安装一次后，Working Memory 会在会话开始时加载，相关记忆会在每轮对话前浮现，并且 Hermes 会在正常结束会话时把清洗后的对话记录同步成 Mem 线程。无需配置 SOUL.md 行为指引。

**在上游合并前**
在这个提供者被 `NousResearch/hermes-agent` 正式接收之前，当前推荐安装路径仍然是本页提供的 Nowledge 社区安装脚本。上游合并后它会成为长期正式入口；在那之前，这份指南会明确保留可用的过渡路径，避免用户卡住。

跨工具的知识，在每次 Hermes 会话中都可用。在 Claude Code 中做的决策、在 Cursor 中设定的偏好、在 ChatGPT 中获得的洞察，汇聚成一个知识图谱，随时可以调用。

**安装成功的标志**
安装插件并重启 Hermes 后，开始一个新会话。Working Memory 应出现在系统提示中。问一句"我最近做了哪些决策？"，Hermes 应该直接搜索你的知识图谱，不需要你指定工具。

## 准备工作

- **Nowledge Mem** 已在本地运行（[安装指南](https://mem.nowledge.co/zh/docs/installation)）或可访问的远程服务器
- **Hermes Agent** v0.7.0+（v0.6.x 可使用 MCP 模式）

```bash
nmem status     # 确认 Nowledge Mem 正在运行
hermes --version # 确认 Hermes 可用
```

## 安装

### 插件安装（推荐）

```bash
bash <(curl -sL https://raw.githubusercontent.com/nowledge-co/community/main/nowledge-mem-hermes/setup.sh)
```

安装原生记忆提供者插件。运行后重启 Hermes。

也可以手动安装：
将插件文件复制到 `~/.hermes/plugins/nowledge-mem/`：

```bash
mkdir -p ~/.hermes/plugins/nowledge-mem
```

### MCP 模式（Hermes < v0.7.0）

```bash
bash <(curl -sL https://raw.githubusercontent.com/nowledge-co/community/main/nowledge-mem-hermes/setup.sh) --mcp
```

这会在 `config.yaml` 中添加 MCP 服务器配置，并将行为指引写入 `~/.hermes/SOUL.md`。工具名称带有 `mcp_nowledge_mem_` 前缀。

MCP 模式下，如果缺少行为指引，Hermes 虽然能访问工具，但不会主动使用。如果 Hermes 能检索记忆却从不主动保存，通常就是指引缺失。插件模式不需要额外的 SOUL 指引，因为这部分提示已经内置在提供者里。

### 验证

问 Hermes 一个依赖过去工作的问题：

> 我最近做了哪些决策？

插件模式下，Hermes 应调用 `nmem_search`。MCP 模式下，应调用 `mcp_nowledge_mem_memory_search`。然后你可以让 Hermes 把结论存成记忆，或者观察它是否会在对话达到稳定结论时主动调用 `nmem_save`。

## 自动化行为

插件接入了 Hermes 的记忆提供者生命周期，以下行为无需手动触发：

- **Working Memory**：在每次会话开始时自动加载
- **相关记忆**：在每轮对话前自动浮现（主动召回）
- **用户画像**：从 Hermes 内置记忆同步到跨工具知识图谱
- **会话记录**：会在 Hermes 正常结束或重置会话时同步成 Mem 线程
- **上下文压缩器**：知晓外部知识的存在，可通过搜索恢复

MCP 模式下，这些行为依赖 SOUL.md 中的行为指引，无法完全保证。

## Hermes 记忆 vs Nowledge Mem

Hermes 自带的记忆系统存储 Hermes 会话中的特定信息。Nowledge Mem 是互补的：它存储跨工具的知识。两者配合使用：

- **Hermes 记忆**：Hermes 特有的偏好、环境信息、工具习惯
- **Nowledge Mem**：决策、流程和经验，未来在任何工具中都应该知道的知识

插件会自动将 Hermes 中的用户画像同步到 Nowledge Mem，让跨工具知识保持一致。

## 你能做什么

- **在 Hermes 中查找其他工具的知识。** 问一句"数据库方案之前定了什么？"，Hermes 会搜索你在 Claude Code、ChatGPT、Cursor 中积累的决策和洞察。
- **保存知识，全局可用。** 在 Hermes 中得出结论后，你可以让它调用 `nmem_save` 保存到知识图谱；下次打开 Claude Code、Cursor 或 ChatGPT 时都能继续用。
- **搜索历史对话。** 按关键词搜索所有工具中的历史对话，支持分页获取完整记录。
- **让 Hermes 会话也可搜索。** 当你正常退出 Hermes、执行 `/new`、`/reset`，或让网关会话正常过期时，提供者会把清洗后的 `user` / `assistant` 对话记录保存成 Mem 线程。第一次会导入整段线程，同一条活跃会话后续再结束时只会追加新增部分。

MCP 模式下还可以使用图谱探索工具，追溯决策的演变过程和发现关联记忆。

## 工具一览

插件模式使用简洁的 `nmem_` 前缀。MCP 模式使用 `mcp_nowledge_mem_` 前缀。

| 插件模式 | MCP 模式 | 说明 |
|----------|----------|------|
| nmem_search | memory_search | 搜索记忆 |
| nmem_save | memory_add | 保存或更新决策、洞察或经验 |
| nmem_update | memory_update | 更新已有记忆 |
| nmem_delete | memory_delete | 删除一条或多条记忆 |
| nmem_thread_search | thread_search | 搜索历史对话 |

## 配置

配置当前机器上的 `nmem` 客户端，让它指向远程服务器：

```bash
nmem config client set url https://your-server:14242
nmem config client set api-key your-key
```

这一步修改的是 Hermes 所在机器的客户端连接配置，不是 Mem 服务器端的 Access Anywhere 或局域网监听配置。

插件唯一的独立配置是请求超时，保存在 `~/.hermes/nowledge-mem.json`：

```json
{
  "timeout": 60
}
```

## Spaces

Hermes 现在支持三种干净的 lane 设计：

- **space**：当前 Hermes profile 固定使用一个 space
- **space_by_identity**：把少量明确身份映射到命名好的 spaces
- **space_template**：如果 Hermes 已经暴露稳定 identity，就按模板派生 space

如果这些都没配置，Hermes 仍然可以继承 `NMEM_SPACE`。

MCP 模式下，直接在 `config.yaml` 中更新地址：

```yaml
# ~/.hermes/config.yaml
mcp_servers:
  nowledge-mem:
    url: "https://your-server/mcp"
    headers:
      Authorization: "Bearer your-key"
    timeout: 120
```

参见 [随处访问 Mem](https://mem.nowledge.co/zh/docs/remote-access)。

## 更新

MCP 工具由 Nowledge Mem 服务器提供，更新桌面应用后自动更新。插件更新需要重新运行安装命令。

## 安全地自定义

优先使用 Hermes 自己的指引文件，而不是去改插件安装目录：

- `~/.hermes/SOUL.md` 适合放个人默认习惯
- 项目根目录的 `HERMES.md` 适合放仓库级规则

不要直接修改 `~/.hermes/plugins/` 下已安装的 Nowledge Mem 插件文件。完整对照表见 [安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## 常见问题

- **无法连接 Nowledge Mem。** 用 `nmem status` 确认服务器正在运行，检查地址是否匹配。
- **Hermes 能检索但从不主动保存持久记忆（MCP 模式）。** 行为指引缺失。运行安装命令后重启 Hermes。指引需要在 `~/.hermes/SOUL.md`（每次会话都加载）或项目级 `HERMES.md`（在 git 根目录）中。插件模式下，检索和会话记录捕获都已经内置在提供者生命周期里，不需要额外指引。
- **Hermes 线程没有出现在 Mem 里。** 这个提供者是在真实会话边界做捕获，不是每轮都写一次。请用正常退出、`/new`、`/reset` 或正常的 gateway 会话过期来验证。如果 Hermes 被强制杀掉，`on_session_end` 可能来不及执行。
- **工具未出现（插件模式）。** 确认 `config.yaml` 中设置了 `memory.provider: "nowledge-mem"`，且插件文件存在于 `~/.hermes/plugins/nowledge-mem/`。重启 Hermes。
- **工具未出现（MCP 模式）。** 确认 `config.yaml` 中有 `mcp_servers.nowledge-mem` 配置块。重启 Hermes。检查 YAML 格式是否正确。
- **响应缓慢。** 默认超时为 30 秒。在 `nowledge-mem.json`（插件模式）或 `config.yaml`（MCP 模式）中调大超时值。如果问题持续，用 `nmem status` 检查服务器状态。

## 相关

- [集成概览](https://mem.nowledge.co/zh/docs/integrations)
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)
- [OpenCode](https://mem.nowledge.co/zh/docs/integrations/opencode)
- [Pi](https://mem.nowledge.co/zh/docs/integrations/pi)
- [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw)
- [Alma](https://mem.nowledge.co/zh/docs/integrations/alma)
- [Bub](https://mem.nowledge.co/zh/docs/integrations/bub)
- 源码：[nowledge-mem-hermes](https://github.com/nowledge-co/community)

---

# Raycast

Nowledge Mem 的 Raycast 扩展。不离开键盘，完成搜索记忆、快速保存和读取工作记忆简报。

**来源：** [community/nowledge-mem-raycast](https://github.com/nowledge-co/community/tree/main/nowledge-mem-raycast)

**Raycast 扩展与 Raycast AI 对话**

本文介绍的是 **Nowledge Mem 的 Raycast 扩展** —— 通过 API 在键盘上操作 Mem。

Raycast AI 对话没有官方整包导出；要把历史迁入「对话」，请使用社区导出工具并通过 Mem [批量导入](https://mem.nowledge.co/zh/docs/import-existing-conversations) 完成。参见 [Raycast AI Exporter 介绍页](https://mem.nowledge.co/zh/integrations/raycast-ai-exporter)、[对话 — 批量导入](https://mem.nowledge.co/zh/docs/threads#bulk-import)。

## 安装

**Raycast Store：** 搜索「Nowledge Mem」或 [直接打开链接](https://www.raycast.com/wey-gu/nowledge-mem) 添加扩展。

**从源码安装**（用于开发或自定义）：

```bash
git clone https://github.com/nowledge-co/community.git
cd community/nowledge-mem-raycast
npm install && npm run dev
```

你可以用两种方式连接：

- **本地默认**：保持 `http://127.0.0.1:14242`
- **远程 Mem**：可以直接在 Raycast 偏好设置中填写 `Server URL` 和 `API Key`。如果你想让 Raycast、CLI 和其他插件共用一套配置，也可以在这台机器上执行：

```bash
nmem config client set url https://mem.example.com
nmem config client set api-key nmem_your_key
```

Raycast 也支持一个可选的 `Space` 偏好设置。适合这种情况：这个 Raycast profile 天然就只对应一个命名好的 lane，例如 `Research Agent`。留空则继续使用 `Default`。

大部分命令都同时支持本地和远程 Mem。只有 `编辑工作记忆简报` 是本地专用的便捷命令，因为它直接编辑你这台机器上的 `Default` Working Memory 文件。

## 命令列表

| 命令 | 功能 |
|------|------|
| 搜索记忆 | 语义搜索，显示相关度分数。搜索框为空时，会展示最近的记忆 |
| 添加记忆 | 保存记忆，设置标题、内容和重要性 |
| 读取工作记忆简报 | 通过 Mem API 读取你的每日简报 |
| 编辑工作记忆简报 | 在本地 Raycast 中编辑 Default Working Memory 文件 |

## 安全地自定义

Raycast 属于启动器型集成，不是那种自带项目级指引文件的 agent runtime。

- 想做稳定修改，优先使用 Raycast 偏好设置，比如 Server URL、API Key 和固定 Space
- 除非你在开发这个扩展，否则不要去改扩展源码

完整对照表见 [安全地自定义集成行为](https://mem.nowledge.co/zh/docs/integrations/customize-behavior)。

## Spaces

对 Raycast 这种启动器型集成，最合适的抽象就是「一个可选的固定 lane」：

- 如果你希望 Raycast 一直待在 `Default`，就把 `Space` 留空
- 如果这个 Raycast profile 始终属于一个稳定上下文，就设置一个命名好的 `Space`
- 不要期待 Raycast 自己去推断"每个 agent 属于哪个 lane"

一旦设置了 Space，`搜索记忆`、`添加记忆`、`读取工作记忆简报` 都会自动跟着这个 lane 走。`编辑工作记忆简报` 则仍然是本地、Default-only 的便捷入口。

## 最推荐的用法

- 想最快找到答案时，用 **搜索记忆**
- 想从键盘里顺手记下一条内容时，用 **添加记忆**
- 想看今天的重点时，用 **读取工作记忆简报**

图谱探索不属于 Raycast 扩展当前的能力范围。需要图谱时，请使用 Mem App，或 Claude Code / Codex 这类支持图谱交互的集成。

## 相关

- [集成概览](https://mem.nowledge.co/zh/docs/integrations)：原生集成、复用包、MCP 与浏览器捕获
- [Claude Code](https://mem.nowledge.co/zh/docs/integrations/claude-code)
- [Claude Desktop](https://mem.nowledge.co/zh/docs/integrations/claude-desktop)
- [Codex CLI](https://mem.nowledge.co/zh/docs/integrations/codex-cli)
- [Alma](https://mem.nowledge.co/zh/docs/integrations/alma)
- [OpenClaw](https://mem.nowledge.co/zh/docs/integrations/openclaw)
- [内置 Web 聊天](https://mem.nowledge.co/zh/docs/integrations/built-in-web-chats)
- [随处访问 Mem](https://mem.nowledge.co/zh/docs/remote-access)

---

# 随处访问 Mem

用安全的 URL + API Key，把任何设备或智能体接入你的 Mem。

Nowledge Mem 运行在你自己的设备上——数据完全在你掌控之中。「随处访问」让你通过安全隧道，从其他任何设备、智能体或浏览器访问同一个实例。

一个 Mem，多种连接方式：咖啡店里的笔记本、手机上的浏览器、CI 上的编程智能体、另一台电脑上的 AI Now。你常开设备上的 Mem 就是中枢，其他一切都连接到它。

这也是 Mem 现在的同步方式：一台常开 Mem，多端接入。如果你想先看简短说明，再回来做配置，可以先读 [多设备同步](https://mem.nowledge.co/zh/docs/sync)。

## 推荐方案

在一台常开设备上运行 Nowledge Mem——Mac Mini、Linux 服务器或始终通电的桌面电脑。然后从其他地方连接：第二台笔记本、浏览器或 iOS 移动应用。这样你可以 24/7 访问知识库、后台智能持续运行、所有工具和设备共享同一数据源。

## 先选连接方式

| 类型 | 适用场景 | 你会得到的 URL |
|------|----------|----------------|
| 快速链接 | 1 分钟内快速可用 | 随机 `*.trycloudflare.com` |
| Cloudflare 账号 | 日常长期稳定使用 | 你自己域名下的固定 URL（如 `https://mem.example.com`） |

## 开始前确认

- **重要** 请从 `设置 → 随处访问 Mem → Guide` 打开本指南。
- 快速链接不需要 Cloudflare 账号，也不需要域名。
- Cloudflare 账号模式要求你已经有一个在 Cloudflare 托管的域名。
- 如果你还没有域名，先使用 **快速链接**。
- 在 Cloudflare 账号模式里，只有创建 hostname route 之后才会出现最终公网 URL。

**nmem TUI 路径**

如果你是服务器 / 终端工作流，可打开 `nmem tui` → `Settings` 标签 → `Access Anywhere`。

你可以在这里配置稳定链接、启动/停止 tunnel、轮换/显示 key、查看终端变量配置。

**TUI 使用注意**
Access Anywhere 的管理接口仅允许本机调用。如果你当前使用的是远程 API（`NMEM_API_URL=https://...`），请先临时切回本机（`http://127.0.0.1:14242`）再做 tunnel 配置。

## 路径 A：快速链接（无需账号）

### 1. 在 Mem 中打开远程访问

### 2. 选择 Quick link 并启动

### 3. 复制 URL 和 API Key

### 4. 在另一台设备验证

```bash
export NMEM_API_URL="https://your-quick-link.trycloudflare.com"
export NMEM_API_KEY="nmem_..."
nmem status
```

预期：`status ok`。

**Linux / 服务器网络说明**

有些 VPS、公司网络、校园网或运营商网络会拦截 UDP/QUIC。现在如果 QUIC 启动失败，Mem 会自动改用 HTTP/2 重试 Cloudflare tunnel。

如果你希望在 headless / systemd 部署里直接强制使用 TCP，可设置：

```bash
export TUNNEL_TRANSPORT_PROTOCOL=http2
```

然后重启 Mem，再重新启动 Access Anywhere。

## 路径 B：Cloudflare 账号（固定 URL）

**前提**
你需要先在 Cloudflare DNS 中管理自己的域名（例如 `example.com`），才能拿到固定 URL。

### 1. 创建 tunnel 并复制 token

在 Cloudflare Zero Trust 中：
- 打开 `Networks` → `Connectors` → `Create a tunnel`
- 点击 `Select Cloudflared`
- 输入 tunnel 名称并点击 `Save tunnel`
- 在 `Install and run connectors` 中，从命令复制 token，例如：

```bash
sudo cloudflared service install ... <TOKEN>
```

在 Mem 桌面端中，你可以粘贴：原始 token；或

### 2. 创建 Public Hostname 路由

### 3. 将 hostname 映射到本机 Mem API

打开 `Networks` → `Connectors` → 你创建的 tunnel。

在 `Published application routes` 点击 `Add a published application route`。

将 `mem.example.com` 映射到本机 Mem 服务：

- **Subdomain**：`mem`
- **Domain**：你在 Cloudflare 托管的域名
- **Service Type**：`HTTP`
- **Service URL**：`http://127.0.0.1:14242`

不要追加 `/remote-api`。

### 4. 回到 Mem 保存并启动

### 5. 在另一台设备验证

```bash
export NMEM_API_URL="https://mem.example.com"
export NMEM_API_KEY="nmem_..."
nmem status
```

预期：`status ok`。

## 在其他客户端使用

先按场景选择最合适的连接入口：

- **移动应用** — 在手机或平板上获得原生 iOS / Android 体验
- **桌面应用** — 从第二台电脑获得完整体验（包括 AI Now）
- **浏览器** — 从任意设备快速访问
- **`~/.nowledge-mem/config.json`** — `nmem config client ...` 写入的共享客户端配置文件，`nmem`、OpenClaw、Bub、Claude Code 等集成都会自动复用
- **浏览器扩展** — 在 SidePanel 设置中粘贴 URL + key
- **直接 MCP** — 用于没有更好专属路径的 MCP 客户端，或像 Codex 这样需要插件包 + MCP 一起配合的 hybrid 宿主

### 移动应用（iOS 和 Android）

**Alpha 版本**

移动应用目前为 Alpha 版本。iOS 通过 TestFlight 提供，Android 通过未签名 APK 下载。请通过 [Discord](https://discord.gg/nowledge) 获取 TestFlight 访问权限，或前往 [社区发布页面](https://github.com/nowledge-co/community/releases/tag/v0.6.11) 下载 APK。

移动应用是一个原生壳应用，连接到你的 Mem 服务器——无需本地数据库或 Python 后端。你可以在手机上使用完整功能：搜索、记忆、对话、文库、知识图谱和动态。

1. 安装应用（iOS 使用 TestFlight，Android 使用 APK）
2. 输入你的 `Mem URL` 和 `API Key`
3. 点击 `连接`
4. 应用会在本地存储你的凭据，后续启动时自动重新连接。

### 桌面应用（客户端模式）

在另一台电脑上安装 Nowledge Mem，然后连接到你的主实例：

1. 打开 `设置 → 随处访问`
2. 输入主 Mem 的 URL 和 API Key
3. 点击 `连接`

你将获得完整的桌面体验——搜索、记忆、对话、文库、知识图谱以及 [AI Now](https://mem.nowledge.co/zh/docs/ai-now)。AI Now 在你正在使用的设备上本地运行，但使用远程服务器上配置的 LLM 提供商，无需在客户端额外设置。

标题栏显示 `远程` 表示你已连接到另一台 Mem，在 AI Now 标签页则显示 `本地 AI` 表示智能体在本机运行。

### 浏览器

在你的 Mem URL 后面加上 `/app` 打开即可——例如 `https://mem.example.com/app` ——支持任何现代浏览器。

输入 API key 登录后，你可以使用搜索、记忆、对话和知识图谱视图。AI Now 不支持浏览器——如需使用 AI Now，请通过上述桌面应用客户端模式连接。

这是从未安装桌面端或移动应用的电脑上查看知识库最快的方式。

在手机上，推荐使用原生移动应用（见上方）。如果你偏好浏览器，点击 `分享 → 添加到主屏幕`（iOS）或安装横幅（Android），即可像快捷方式一样全屏打开，无需浏览器边框。

### nmem CLI

同一台电脑无需手动配置。

如果 `nmem` 与桌面应用运行在同一台电脑上，当"随时访问"生成 API 密钥时会自动创建此文件。你可以直接运行 `nmem status`。

先在这台机器上执行一次，之后所有 `nmem` 命令——以及所有使用 `nmem` 的插件——都会自动连接：

```bash
nmem config client set url https://<your-url>
nmem config client set api-key nmem_...
nmem status  # 自动读取 config.json
nmem m search "project notes"
```

这条命令写入的是当前机器的本地客户端连接配置。OpenClaw、Bub、Claude Code、Claude Desktop 等集成都直接复用它。

### 浏览器扩展（SidePanel）

### OpenClaw 插件

如果你已经在这台机器上执行过 `nmem config client ...`，OpenClaw 插件会自动读取同一份共享客户端配置。

```json
// ~/.nowledge-mem/config.json
{
  "apiUrl": "https://<your-url>",
  "apiKey": "nmem_..."
}
```

也可以在 OpenClaw 仪表盘的 `Automation → Plugins → Nowledge Mem` 中配置。

API key 只通过环境变量传给 `nmem` 子进程，不会出现在日志或命令行参数里。插件附带的行为技能（如 memory guide）在远程模式下照常工作——它们是插件的一部分，不依赖服务端。

### Bub 插件

如果你已经在这台机器上执行过 `nmem config client ...`，Bub 插件会自动读取同一份共享客户端配置。

```json
// ~/.nowledge-mem/config.json
{
  "apiUrl": "https://<your-url>",
  "apiKey": "nmem_..."
}
```

也可以在启动 Bub 前设置 `NMEM_API_URL` 和 `NMEM_API_KEY` 环境变量。

### Alma 插件

两种方式都可以，选你顺手的：

**方式 A：插件设置（推荐）**

在 Alma 中打开设置，配置 Nowledge Mem 插件：
- `nowledgeMem.apiUrl`：远程 URL（如 `https://mem.example.com`）。留空则使用本机。
- `nowledgeMem.apiKey`：Mem API key（`nmem_...`）。仅通过环境变量传递，不会出现在日志或命令行参数中。

插件在激活日志中会显示 `mode=remote` 或 `mode=local`，方便确认当前模式。

**方式 B：环境变量**

在启动 Alma 前设置：

```bash
export NMEM_API_URL="https://<your-url>"
export NMEM_API_KEY="nmem_..."
```

两种方式效果相同。想让配置自成一体用方式 A；想把密钥放在配置文件之外用方式 B。

### MCP / 智能体节点

MCP 客户端通过 HTTP 连接，需要在 `Authorization` 请求头中传入 API key，或者用 `X-NMEM-API-Key` 头传入。

请使用带结尾斜杠的精确 MCP 地址：`https://<your-url>/mcp/`。

**Cursor**（`~/.cursor/mcp.json` 或工作区 `.cursor/mcp.json`）：

```json
{
  "mcpServers": {
    "nowledge-mem": {
      "url": "https://<your-url>/mcp/",
      "type": "streamableHttp",
      "headers": {
        "APP": "Cursor",
        "Authorization": "Bearer nmem_..."
      }
    }
  }
}
```

或者

```json
{
  "mcpServers": {
    "nowledge-mem": {
      "url": "https://<your-url>/mcp/",
      "type": "streamableHttp",
      "headers": {
        "APP": "Cursor",
        "X-NMEM-API-Key": "nmem_..."
      }
    }
  }
}
```

**Claude Desktop**

如果你使用 [Nowledge Mem 扩展](https://mem.nowledge.co/zh/docs/integrations/claude-desktop)，它会直接读取和 `nmem` 一样的共享客户端配置：

- macOS / Linux: `~/.nowledge-mem/config.json`
- Windows: `%USERPROFILE%\.nowledge-mem\config.json`

```json
{
  "apiUrl": "https://<your-url>",
  "apiKey": "nmem_..."
}
```

**Claude Code**

安装 [Nowledge Mem 插件](https://mem.nowledge.co/zh/docs/integrations/claude-code) 即可获得自动工作记忆简报、搜索和会话捕获。远程模式下，在客户端机器上执行一次 `nmem config client set url ...` 和 `nmem config client set api-key ...` 即可——插件中的 `nmem` 命令会自动读取这份共享客户端配置。

**CI / 其他基于 Shell 的工具**

设置 `NMEM_API_URL` 和 `NMEM_API_KEY` 环境变量即可。

**远程模式下，真实会话导入仍然发生在客户端本机**

对于 `nmem t save --from claude-code`、`gemini-cli`、`codex` 这类基于真实会话记录的保存，远程模式并不意味着 Mem 服务器会去远程读取这些智能体的会话文件。真正的本地发现与解析，仍然发生在运行该智能体的那台客户端机器上，然后再把规范化后的线程数据上传到 Mem。

## 快速健康检查

```bash
curl -H "Authorization: Bearer $NMEM_API_KEY" "$NMEM_API_URL/health"
```

预期：返回健康检查 JSON。

错误 key 检查：

```bash
curl -H "Authorization: Bearer wrong_key" "$NMEM_API_URL/health"
```

预期：`401`。

如果代理会剥离鉴权头：

```bash
curl "$NMEM_API_URL/health?nmem_api_key=$NMEM_API_KEY"
```

## 安全与运行建议

- 所有远程请求都需要 API key，包括 tunnel 和局域网连接。
- 开启局域网访问后，同一 Wi-Fi 上的其他设备连接时需要 API key。来自这台电脑本身的请求始终免 key，除非你在设置中开启了 `Require API key on localhost`。
- 可随时在设置中 `Rotate`（旧 key 立即失效）。
- 首次成功 `Start` 后，应用重启会自动重连，直到你点击 `Stop`。
- Browse-Now / Browser Bridge 自动化端点仅限本机访问，不会通过「随处访问 Mem」暴露。
- 不需要远程访问时请关闭 tunnel。

## 常见问题

- **Start 超时**：网络/代理可能拦截了 Cloudflare 流量，重试或切换到账号模式。
- **401 Missing API key**：通常是代理移除了鉴权头。升级 `nmem`，或手动使用 query 回退验证。
- **429 Too many invalid auth attempts**：错误 key 被连续重试。重新复制 key 或点击 `Rotate`。

## 相关

- [Raycast](https://mem.nowledge.co/zh/docs/integrations/raycast)
- [多设备同步](https://mem.nowledge.co/zh/docs/sync)

---

# 多设备同步

现在的 Nowledge Mem 是如何实现同步的：一台常开 Mem，多端接入

可以。Nowledge Mem 现在已经支持同步。

但它的同步方式很明确：
- 以一个 Nowledge Mem 实例作为**唯一事实来源**
- 其他客户端连接到这一台 Mem
- 记忆、线程、图谱、文库之所以保持一致，是因为大家访问的是同一个后端

这和"每台设备各跑一份独立数据库，之后再互相合并"不是一回事。

## Mem 里的同步到底指什么

推荐的方式很简单：

1. 在一台常开机器上运行 Nowledge Mem。
2. 打开 [随处访问](https://mem.nowledge.co/zh/docs/remote-access)。
3. 让其他客户端连接到这同一个 Mem。

这些客户端可以是：
- 另一台电脑上的桌面应用
- `/app` 网页版
- 移动应用
- 浏览器扩展
- `nmem` CLI
- 运行在其他机器上的受支持智能体集成

只要它们都连接到同一个 Mem URL 和 API Key，它们看到的就是同一个知识库。

## 这不是什么

Mem 目前的同步**不是**：
- 多台设备上各自独立运行的 Mem 自动互相复制、合并
- 由 Nowledge 托管的中心化账号后端
- 多个独立数据库之间的离线优先、多主同步

现在的同步模型，本质上是：**一台 Mem 中枢，多端接入**。

## 什么时候这种方式最合适

如果你本来就有下面这类设备，这种方式会非常合适：
- 一台长期开机的 Mac Mini
- 一台 Linux 服务器
- 一台作为主知识中枢的桌面电脑
- 一台专门跑 OpenClaw 或其他智能体的机器

你只需要让 Mem 在那台机器上长期运行，其他设备再来连接它。

**为什么很多用户以前以为 Mem 不能同步**

过去大家最常看到的远程入口主要还是浏览器扩展和 `nmem` CLI，所以很容易误以为 Mem 只是"能远程访问一点点"，而不是真正支持同步。这个判断已经过时了。现在网页访问、桌面客户端模式和移动应用都已经补上了这一层。

## 接下来读什么

- 看 [随处访问](https://mem.nowledge.co/zh/docs/remote-access)：了解具体配置、安全要求、Cloudflare tunnel 步骤，以及各类客户端怎么接入。
- 看 [AI Now](https://mem.nowledge.co/zh/docs/ai-now)：如果你想在另一台电脑上远程使用同一个知识库里的 AI Now。
- 看 [如何确认 Mem 已经在工作](https://mem.nowledge.co/zh/docs/verify-it-works)：确认另一台设备是否真的已经连到同一个 Mem。

---

# Nowledge Mem 用例概述

Nowledge Mem 会把你和 AI 一起完成的工作慢慢沉淀下来。它会通过适合各个工具的路径捕获对话，让会话和记忆都变得可搜索，并在后台持续整理知识图谱。已连接的工具也能带着更完整的上下文开始工作。

先选一个你最想解决的问题来看。第一天并不需要把 Mem 的所有能力都理解完。

## 用例卡片

### [你的知识，你做主](https://mem.nowledge.co/zh/docs/use-cases/shared-memory)
告诉 Claude 一次，Cursor 也能接上。一个知识库，覆盖你连接到 Mem 的 AI 工具。

### [永不丢失会话](https://mem.nowledge.co/zh/docs/use-cases/session-backup)
原生保存路径、本地自动同步和浏览器捕获，让重要对话持续可搜索。

### [穿越时间搜索](https://mem.nowledge.co/zh/docs/use-cases/bi-temporal)
董事会问为什么选了 React Native。找到你当时相信的，而不是你现在知道的。

### [你的笔记，无处不在](https://mem.nowledge.co/zh/docs/use-cases/notes-everywhere)
Obsidian、Notion、PDF、Word 文档。一次搜索覆盖所有知识源。

### [看见你的专长](https://mem.nowledge.co/zh/docs/use-cases/expertise-graph)
图谱从你的记忆中自动构建。社区检测揭示你不知道自己拥有的专长集群。

### [AI Now](https://mem.nowledge.co/zh/docs/ai-now)
运行在本地的个人 AI 智能体。它可以使用你保存的知识、文件和已启用的插件来完成深度研究、文件分析和演示文稿。

## Mem 改变了什么

浏览器扩展会从受支持的 Web AI 聊天平台抓取洞察和对话备份。本地编程会话可以自动同步，原生集成会在支持的工具里提供专属保存路径，Timeline 也始终支持直接输入和保存。走对捕获路径，你不用操心。

你不整理时，Mem 也会继续整理。后台智能检测思路变化，把零散记忆整理成参考内容，标记矛盾。每天早上的 Working Memory 简报会在你开口之前先把重点告诉 AI 工具。

原生集成、复用型工作流包和直接 MCP 都连接到同一个记忆系统。切换工具，知识不变。

## 工作原理

1. **捕获** -- 浏览器捕获、原生保存路径、本地会话同步，或直接输入 Timeline
2. **连接** -- 系统将它关联到你已有的所有知识
3. **生长** -- 后台智能在夜间构建演化链、知识结晶和标记
4. **使用** -- 已连接的工具在需要时可以找到它

知识积累在 Mem 里，不依赖任何单个工具。

## 开始

如果上面的某个问题正好就是你现在的痛点，就从那张卡片开始；如果还不确定，请先回到 [从这里开始](https://mem.nowledge.co/zh/docs/start-here) 或 [快速上手](https://mem.nowledge.co/zh/docs/getting-started)，先跑通一个小闭环。

---

# 你的知识，你做主

知识属于你，不属于任何工具。原生集成、复用型工作流包，以及每日简报，让已连接的 AI 工具保持上下文连续。

## 问题所在

上周你告诉 Claude Code 项目架构。今天，又要向 Cursor 解释一遍。明天，想试试大家都在说的新工具，但又得从零开始。

问题不在于你记不住，而在于知识被绑在了上一个工具里。

> "我已经解释过了。为什么换个工具就得重来？"

## 解决方案

Nowledge Mem 是位于你和 AI 工具之间的一层知识系统。它会自动捕获你的洞察，把会话和记忆汇到同一个可搜索的地方，并生成每日简报，让受支持的工具从更完整的上下文开始。

关键不是所有工具都走同一条安装路径，而是它们最终连接到同一个记忆系统。

**先证明一次就够了**

如果你现在最想解决的就是"换工具就得重讲一遍"，那就先做一个最小验证：在 Mem 里保存一个决策，接上你最常用的工具，然后过一会儿换第二个工具来问同一个主题。只要它能找回之前那条决策，这个闭环就已经成立了。

## 工作原理

### 1. 为你的工具选择合适的连接方式

### 2. 捕获会自然发生

- **浏览器扩展 (Exchange v2)**：扩展监控你在受支持的 Web AI 聊天平台上的对话。洞察会在你工作时自动捕获。
- **会话捕获与同步**：本地编程会话可以实时自动同步。原生集成也会在宿主支持时提供专属的真实会话保存路径。
- **Timeline 输入**：输入一个想法，粘贴一个 URL，拖入一个文件。用于你想保存特定内容的时候。
- **手动命令**：
  - `/sum` -> 将此对话总结成记忆
  - `/save` -> 根据当前集成执行对应的保存或交接摘要动作

### 3. 已连接的工具知情启动

每天早上，后台智能会写出一份 Working Memory 简报。已连接的工具会在各自的接入路径里，于会话开始时读取当前分区对应的那一份。

你的智能体已经知道：
- 你正在做什么
- 你最近做了什么决策
- 开放的问题和矛盾
- 你的思维如何演变

不需要重新解释。早上 9 点打开 Claude Code，它从你上次离开的地方继续。

### 4. 自由切换工具

有新工具？按它支持的最佳方式连接到 Mem，它就可以使用同一套共享上下文。

**示例：**

你保存了："架构决定：使用 Redis 进行会话管理因为..."

后来，在 Cursor 中："帮我添加会话处理"

Cursor 搜索你的知识，找到 Redis 决定，应用相同模式。无需重新解释。

## 实际示例

**没有 Nowledge Mem：**

你："帮我实现限流"
Claude："什么类型？令牌桶？滑动窗口？你的用例是什么？"
你：[这个月第5次解释]

**有 Nowledge Mem：**

你："帮我实现限流"
Claude：[读取工作记忆简报，搜索你的记忆]
"根据你上个月对支付服务使用滑动窗口限流的决定，这是一个匹配你 Redis 模式的实现..."

## 连接方式

| 渠道 | 如何工作 | 捕获什么 |
|------|----------|----------|
| 原生集成 | 工具专属包 | 工作记忆简报、检索、提炼，以及该工具对应的保存路径 |
| 复用型工作流包 | 共享提示词或技能 | 工作记忆简报、搜索、保存、提炼 |
| 浏览器扩展 | 自动捕获 AI 对话 | 来自受支持的 Web AI 聊天平台的洞察 |
| 会话自动同步 | 实时监控 | Claude Code、Cursor、Codex、OpenCode 会话 |
| MCP | 直接协议连接 | 任何兼容 MCP 的工具 |
| Claude Desktop | 一键扩展 | 完整集成 |
| 内置支持 | 在设置中切换 | DeepChat、LobeHub |

## 用得越久，切换越轻松

用几周后，新接入的工具很快就能理解你的工作方式。偏好跨工具延续，决策持续累积，保存过的洞察也能被将来使用的工具找到。

价值积累在 Mem 里，不在任何单个工具上。

## 下一步

- [永不丢失会话](https://mem.nowledge.co/zh/docs/use-cases/session-backup) -> 自动同步和备份 AI 对话
- [穿越时间搜索](https://mem.nowledge.co/zh/docs/use-cases/bi-temporal) -> 找到你当时知道的
- [集成](https://mem.nowledge.co/zh/docs/integrations) -> 连接所有工具

---

# 永不丢失会话

原生保存路径、本地自动同步和浏览器捕获，让重要 AI 对话持续可搜索。

## 问题所在

你刚刚进行了一次史诗般的调试会话。与 Claude Code 三个小时。你发现了一个竞态条件，追踪了15个文件，构建了一个带测试的完美修复。

但 AI 对话是短暂的。上下文被压缩，token 限制到达，会话过期。200 条消息的对话线程中，早期内容已经消失了。

> "我以前解决过这个完全相同的问题。我只是不记得怎么解决的了。或者在哪里。或者什么时候。"

## 解决方案

你的会话可以通过适合各个工具的路径进入 Mem。本地编程会话可以自动同步。原生集成会在宿主支持时保存真实会话记录。ChatGPT、Claude 和 Gemini 的浏览器对话则由扩展直接捕获。只有当你手上本来就是导出文件时，才需要走导入路径。

准备好之后，将对话线程提炼成永久、可搜索、连接图谱的记忆。

**先做一次最小验证**

选一段你已经在意的对话，让它进入 Threads，然后从里面提炼出一条真正有用的记忆。只要之后你既能找回原始对话，又能搜索到那条提炼后的记忆，这条工作流就已经成立了。

## 工作原理

### 1. 会话会通过不同路径进入 Mem

**本地自动同步（Claude Code、Cursor、Codex、OpenCode）：**
Nowledge Mem 可以实时监控本地编程会话。打开 `对话` 就能看到它们在你工作时持续出现。

**通过工具专属路径保存真实会话（Claude Code、Gemini CLI、Codex CLI）：**
有些工具会通过自己的专属路径导入真实录制下来的会话。Claude Code 和 Gemini 走原生集成；Codex 则走它自己的插件包，并通过

### 2. 提炼成永久知识

打开保存的对话线程并点击 `提炼`。AI 阅读整个对话并提取：

- **决定**："选择滑动窗口而不是令牌桶因为..."
- **洞察**："异步回调中的竞态条件需要互斥锁"
- **模式**："测试基于时间的 bug 需要模拟时钟"
- **事实**："Redis SETNX 提供原子锁获取"

每个都成为独立的、可搜索的记忆，带有适当的标签。

### 3. 后台智能自动连接

你的新记忆不会孤立存在。后台智能会：
- 将它们链接到同一代码库的以前工作
- 检测它们是否更新或矛盾了早期决策
- 将它们连接到知识图谱中的相关实体
- 在第二天早上的工作记忆简报中浮现

三个月后，同事遇到同样的 bug。你的简报在他们开口之前就提到了它。

### 4. 随时搜索

三个月后，类似的 bug 出现：

搜索："支付竞态条件"

Nowledge Mem 返回完整上下文：问题、调试步骤、解决方案、测试方法。

不再重新解决已解决的问题。

## 捕获来源

| 来源 | 方式 | 捕获内容 |
|------|------|----------|
| Claude Code | 原生插件保存或本地自动同步 | 带代码上下文的完整会话 |
| Gemini CLI | 原生扩展 `save-thread` | 真实录制的 Gemini 会话 |
| Droid | 原生插件 `save-handoff` | Droid 内的可恢复交接摘要，并明确不把它包装成完整会话导入 |
| Codex | 工具专属 `/save` 工作流或本地自动同步 | 带代码上下文的完整会话 |
| Cursor | 插件 `save-handoff`、本地自动同步或手动导入 | 插件中的可恢复交接摘要，以及你机器上的本地对话导入 |
| OpenCode | 自动同步（实时监控） | 对话实时捕获 |
| ChatGPT | 浏览器扩展（自动捕获） | 网页聊天中的洞察与完整对话备份 |
| Claude Web | 浏览器扩展（自动捕获） | 网页聊天中的洞察与完整对话备份 |
| Gemini | 浏览器扩展（自动捕获） | 网页聊天中的洞察与完整对话备份 |
| 更多受支持的 Web AI 聊天平台 | 浏览器扩展 | 在受支持的网站上使用同样的捕获模型 |

## 提取的内容

当你提炼对话线程时，AI 按类型创建记忆：

| 类型 | 示例 | 标签 |
|------|------|------|
| 决定 | "使用 Redis 进行分布式锁" | 决定、架构 |
| 洞察 | "异步回调需要仔细排序" | 洞察、调试 |
| 过程 | "重现竞态条件的步骤" | 过程、测试 |
| 事实 | "SETNX 如果键被设置返回 1" | 事实、redis |
| 经验 | "支付服务的调试会话" | 经验、项目 |

## 时间的积累

一个对话线程有用，十个是知识库，一百个就是你的机构记忆。

> "今天初级开发者遇到了同样的 bug。发给他们我的记忆。他们20分钟修复了，而不是3小时。"

调试会话不只是对话，而是给未来自己的可复用知识。

## 专业提示

- **有选择地提炼**：你不需要提炼每个对话线程。保存重要的会话：突破、架构决定、来之不易的解决方案。
- **保存前审查**：对于敏感代码库，审查你正在保存的内容。对话线程可能包含专有代码或凭据。

## 下一步

- [你的知识，你做主](https://mem.nowledge.co/zh/docs/use-cases/shared-memory) -> 自由切换工具，不丢失上下文
- [穿越时间搜索](https://mem.nowledge.co/zh/docs/use-cases/bi-temporal) -> 从特定时间段找到记忆
- [集成](https://mem.nowledge.co/zh/docs/integrations) -> 每个工具的设置指南



---

# 第六部分：进阶使用场景与核心概念

# 穿越时间搜索

找到你当时知道的，而不仅仅是你现在知道的。双时态搜索、知识演化追踪和图谱时间线滑块。

## 问题所在

董事会问："为什么你在第一季度选择了 React Native 而不是 Flutter？"

你记得那个决定。但你记得的是通过之后发生的一切的镜头：转型、性能问题、重写。

你需要回答：你当时知道什么？

> "我可以搜索我的笔记中的'React Native'。但我不能搜索'我在三月份对 React Native 的看法'。"

## 解决方案

Nowledge Mem 使用双时态搜索：两个时间维度让你准确找到你要找的东西。

- **事件时间**：事情实际上是什么时候发生的？
- **记录时间**：你什么时候捕获的？

可以单独搜索，也可以组合使用。

## 工作原理

### 自然语言查询

只需自然地搜索。Nowledge Mem 理解时间意图：

> "我在 2024 年第一季度对 React Native 做了什么决定？"

系统：
1. 检测时间意图："2024 年第一季度"
2. 搜索**事件**发生在该期间的记忆
3. 返回带有原始上下文的结果

不需要特殊语法。

### 显式时间过滤器

对于精确控制，使用高级搜索：

| 过滤器 | 含义 | 示例 |
|--------|------|------|
| 事件日期从 | 事件发生在此之后 | 2024-01-01 |
| 事件日期到 | 事件发生在此之前 | 2024-03-31 |
| 记录日期从 | 写下在此之后 | 2024-01-01 |
| 记录日期到 | 写下在此之前 | 2024-12-31 |

强大查询示例：
- 事件时间：2024 年 3 月
- 记录时间：任何
- 返回："所有关于 2024 年 3 月事件的记忆，无论你什么时候记录的。"

### 灵活的日期精度

Nowledge Mem 处理灵活的日期：

- **年**："2024" -> 匹配 2024 年的任何内容
- **月**："2024-03" -> 匹配 2024 年 3 月
- **日**："2024-03-15" -> 匹配那个特定日期

系统保留你的原始精度并相应显示。

### 知识演化

双时态搜索与知识演化结合更加强大。后台智能自动检测你对某个话题的想法变化：

- **周二**：你保存了"新服务用 PostgreSQL。"
- **周四**：你提到 CockroachDB 作为迁移目标。
- **周五**：后台智能用 EVOLVES 关系链接它们，标记出张力。

现在搜索"数据库决策"，你不只是得到孤立的记忆，而是得到演化链：原始决策、更新，以及它们之间的关系。你能准确看到你的思维何时、如何改变。

**演化类型**：

- **替换**：新信息使旧信息过时
- **丰富**：新信息为旧信息添加细节
- **确认**：来自不同来源的相同结论
- **挑战**：矛盾的信息，标记待审查

## 实际示例

### 董事会回顾

- **查询**："2024 年第一季度的架构决定"
- **结果**：带有第一季度上下文的原始决策备忘录，加上展示决策如何变化的演化链

### 合规审计

- **查询**："事故前的安全策略"
- **结果**：违规前存在什么策略，带有证明何时记录的时间戳

### 项目复盘

- **查询**："项目启动时的 project-x 假设"
- **结果**：后来被证明错误的原始假设，链接到证明它们错误的后续洞察

## 知识图谱 + 时间

图谱视图有一个**时间线滑块**，可以按日期范围过滤节点和边。

将范围设置为"2024 年 3 月"并查看：
- 只有当时存在的实体
- 只有当时已知的连接
- 你在那个时刻的知识状态

向前拖动滑块，观察你的理解如何演变。播放动画，看知识随时间累积。

## 记忆衰减如何工作

记忆衰减遵循以下规则：
- 默认**优先最近的记忆**（30 天半衰期）
- **提升经常访问的**记忆（对数缩放）
- **尊重重要性分数**（重要性底线防止完全衰减）
- **从行为中学习**（点击、停留时间）

普通搜索会浮现新鲜、相关的结果；时间搜索则绕过衰减，精确返回你指定的时段。

### 深度模式

时间意图检测需要**深度模式**搜索。在快速模式下，时间引用仅按关键词匹配。对于"最近在做"或"上季度的决定"等查询，启用深度模式。

查看[搜索与相关性](https://mem.nowledge.co/zh/docs/search-relevance)了解评分、衰减和时间匹配如何工作的完整技术分解。

## 两种时间

理解区别是关键：

| 问题 | 哪种时间？ |
|------|-----------|
| "我三月份做了什么决定？" | 事件时间 |
| "我上周写了什么？" | 记录时间 |
| "显示关于旧事件的最近笔记" | 两者 |
| "转型前我知道什么？" | 事件时间 |

大多数搜索使用**事件时间**，因为你在问事情何时发生。

**记录时间**对以下有用：
- 查找最近的捕获
- 审查你一直在记录什么
- 审计知识何时被记录

## 为什么这很重要

传统搜索找内容。时间搜索找上下文。知识演化找故事。

> "我们用当时掌握的信息做了最好的决定。这就是证据。这里是我们的思维何时以及为何改变的完整记录。"

你的记忆带时间戳、有版本控制、历史可查。

## 下一步

- [你的知识，你做主](https://mem.nowledge.co/zh/docs/use-cases/shared-memory) -> 自由切换工具，不丢失上下文
- [看见你的专长](https://mem.nowledge.co/zh/docs/use-cases/expertise-graph) -> 可视化你的知识
- [后台智能](https://mem.nowledge.co/zh/docs/advanced-features) -> 知识图谱能力

---

# 你的笔记，无处不在

Obsidian、Notion、PDF、Word 文档，一次搜索覆盖所有。保留你的工具，接入你的知识。

## 问题所在

你多年来一直在做笔记。Obsidian。Notion。也许两个都用。

数千条记录，仔细标记，广泛链接。然而，

> 我知道我写过这个。我只是找不到它。搜索没用。标签没用。

更糟的是，AI 助手根本不知道这些笔记存在，你每次都在重复解释笔记里已有的内容。

## 解决方案

不取代笔记应用，而是将它接入你的知识。

继续使用 Obsidian、Notion、Apple Notes 或 Markdown 文件夹，就像你现在做的那样。Nowledge Mem 会把它们接入统一的知识层，让你的笔记在 AI Now 中可搜索，也能和你连接到 Mem 的其他 AI 工具一起使用。

有了[资料库](https://mem.nowledge.co/zh/docs/library)，你还可以拖入 PDF、Word 文档和演示文稿。所有内容都从一个地方搜索。

## 工作原理

### 1. 连接你的笔记

**Obsidian**：
- 在 Nowledge Mem 中打开 AI Now
- 前往**插件** -> 启用 **Obsidian**
- 设置你的知识库路径（例如，`/Users/you/Documents/ObsidianVault`）
- 完成。AI Now 现在可以搜索你的知识库

**Notion**：
- 打开 AI Now -> **插件** -> 启用 **Notion**
- 点击**连接 Notion**
- 在浏览器弹出窗口中授权访问
- 你的工作区现在可访问

### 2. 将文档导入资料库

将文件直接拖入 Timeline 输入框或打开资料库视图：

| 格式 | 扩展名 | 处理方式 |
|------|--------|----------|
| PDF | .pdf | 提取文本，分段，索引 |
| Word | .docx, .doc | 解析为文本，分段，索引 |
| 演示文稿 | .pptx | 提取幻灯片内容并索引 |
| Markdown | .md | 直接解析并索引 |

索引完成后，文档内容可与你的记忆和笔记一起搜索。

### 3. 跨所有内容搜索

向 AI Now 提问任何问题：

> 我的笔记关于量子计算说了什么？

AI Now：
- 搜索你的 Obsidian 知识库
- 搜索你的 Notion 工作区
- 搜索你的 Nowledge 记忆
- 搜索你的资料库文档
- 组合并综合结果

一个问题，覆盖所有知识源。

### 4. 提炼成记忆

找到有价值的笔记？将它们转变为永久记忆：

> 从这些量子计算笔记中提炼关键洞察

AI Now 创建：
- **洞察**："量子纠错需要 O(n^2) 量子比特"
- **决定**："近期研究专注于 NISQ 算法"
- **事实**："IBM 在 2023 年 12 月宣称量子优势"

这些记忆现在：
- 可通过语义理解搜索
- 在知识图谱中连接
- 可以被你连接到 Mem 的 AI 工具共同使用
- 相关时会出现在你的工作记忆简报中

## Obsidian 集成

### 设置

1. **打开 Nowledge Mem**
   - 打开 Nowledge Mem 应用。

2. **点击 AI Now 标签**
   - 在侧边栏中选择 **AI Now** 标签。

3. **前往插件**
   - 在侧边栏中打开 **插件**。

4. **启用 Obsidian**
   - 找到 **Obsidian** 并切换开启。

5. **输入知识库路径**
   - 示例：`/Users/yourname/Documents/ObsidianVault`

### 你可以做什么

连接后：
- 按内容搜索笔记："找到我关于机器学习的笔记"
- 阅读特定笔记："显示我关于项目启动的笔记"
- 在上下文中引用："基于我关于 X 的 Obsidian 笔记，帮我..."

**隐私优先**：你的知识库在本地读取。笔记永远不会上传到任何地方。Nowledge Mem 只是读取你机器上的文件。

## Notion 集成

### 设置

1. **打开 AI Now 插件**
   - 打开 AI Now -> **插件**。

2. **连接 Notion**
   - 找到 **Notion** 并点击**连接**。

3. **在浏览器中授权**
   - 在浏览器弹出窗口中完成授权。

4. **选择工作区**
   - 选择你想连接的工作区。

### 你可以做什么

搜索你的工作区：
- "找到关于季度规划的页面"
- "我的产品路线图页面里有什么？"
- "比较我的 Notion 笔记与我关于 X 的记忆"
- 结合公开信息和私人知识进行深度研究："量子计算的最新进展是什么？"

**OAuth 连接**：Notion 使用安全的 OAuth。你完全控制 Nowledge Mem 可以访问哪些页面。随时从 Notion 设置中撤销。

## 内置集成

部分工具已内置 Nowledge Mem：
- **DeepChat**：在设置中开启 Nowledge Mem。你的记忆在每次对话中可用。
- **LobeHub**：从市场安装。完整 MCP 集成。
- **Apple Notes（macOS）**：启用 AI Now 内置插件后即可搜索你的 Apple Notes。

加入[社区](https://mem.nowledge.co/zh/docs/community)请求集成。

## 下一步

- [AI Now](https://mem.nowledge.co/zh/docs/ai-now) -> 了解 AI Now 还能做什么
- [资料库](https://mem.nowledge.co/zh/docs/library) -> 导入和搜索文档
- [看见你的专长](https://mem.nowledge.co/zh/docs/use-cases/expertise-graph) -> 可视化你的知识图谱
- [集成](https://mem.nowledge.co/zh/docs/integrations) -> 完整设置指南

---

# 看见你的专长

知识图谱自动构建。后台智能提取实体、检测社区，追踪专长随时间的演变。

## 问题所在

你多年来积累了大量知识，但能看到它的全貌吗？

> 我知道我擅长...某些东西。技术方面。但如果有人让我描述我的专长，我会很难说清楚。全凭直觉。没有具体的东西。

知识分散在记忆、笔记和对话中，模式和连接都看不见。

## 解决方案

Nowledge Mem 将你的知识可视化为一个**活的图谱**。节点是你的记忆和实体。边是关系。图谱**自动构建**：后台智能在夜间自动从你的记忆中提取实体和关系。

运行**社区检测**，观察你的专长集群浮现。

## 工作原理

### 1. 图谱自动构建

你不需要手动标记或分类任何东西。后台智能读取你的记忆并提取：
- **实体**：技术、人员、概念、项目
- **关系**：它们之间如何连接
- **演化链**：你对某个话题的想法如何变化

这一切自动发生。通过任何渠道保存记忆（自动同步、浏览器扩展、Timeline、`/sum`），图谱就会自行生长。

**使用条件**：自动实体提取需要已配置的远程 LLM，以及你的当前版本所对应的许可能力。

### 2. 运行社区检测

在右侧面板中，找到**图算法**并点击**聚类**下的**计算**。

Louvain 算法分析你的知识结构并找到自然集群：

| 社区 | 大小 | 主题 |
|------|------|------|
| 分布式系统 | 87 条记忆 | 后端架构、扩展 |
| 团队领导 | 45 条记忆 | 指导、沟通 |
| 性能 | 62 条记忆 | 优化、分析 |
| 个人项目 | 23 条记忆 | 创意实验 |

每个集群在其节点周围获得一个彩色"气泡"。

### 3. 穿越时间

图谱底部的**时间线滑块**允许你按日期范围过滤。

拖到"2024 年 1 月"，查看你当时的知识状态。向前拖动，观察新集群形成、现有集群增长、连接增多。

播放动画，观看你的专长在数月间演变。看到新兴趣何时出现，何时与现有知识连接，何时成长为完整的集群。

### 4. 探索和发现

导航图谱：
- **点击**任何节点查看其详情
- **双击**扩展邻居
- **Shift+拖动**套索选择多个节点
- **按 C** 切换社区气泡
- **按 E** 扩展所选节点的邻居

发现你从未注意到的模式：
> 每条领导力记忆都链接回调试会话。我通过教调试来领导。

## 你将发现什么

### 专长集群

社区检测揭示你的知识自然分组的地方：
- **核心优势**：大型、密集的集群
- **新兴领域**：小但正在增长的集群
- **桥梁**：连接多个集群的节点（往往是你最独特的技能）

### 知识演化

后台智能追踪你的思维如何变化：
- **周二**："新服务用 PostgreSQL"
- **周四**："考虑用 CockroachDB 迁移"
- **周五简报**："你的数据库选型在演变"

这些演化链在图谱中显示为链接的节点。你能准确看到你的观点在哪里发生了转变，并追踪整个过程。

### 隐藏模式

探索并发现：
- 你从未有意识追踪的重复主题
- 看似无关的项目之间的连接
- 你独特的视角和方法
- 相关主题之间的差距

### 向 AI 询问你的图谱

查看你的图谱，让 AI Now 解释它：

> 基于我的知识图谱，什么职业道路最适合我？

AI Now 综合：
> 你的记忆显示深度系统知识与教学能力的独特交叉。你最核心的概念（事件驱动架构、调试）连接技术和领导力集群。考虑：Staff Engineer、Developer Advocate 或具有技术重点的 Engineering Manager。

其他可尝试的问题：
- "我最强的专长领域是什么？"
- "我的知识差距在哪里？"
- "接下来我应该探索什么主题？"
- "我的重点是如何随时间变化的？"

## 时间的积累

记忆越多，图谱越丰富，洞察越深。

**1 个月后**：我可以看到我的主要主题，但集群很小

**6 个月后**：清晰的专长领域。意外的连接正在浮现。后台智能在发现我漏掉的模式。

**1 年后**：我可以实际看到我的思维是如何演变的。我去年建立的连接为今年奠定了基础。

对于绩效评估：
> 我在评估前探索了我的图谱。在每个维度都有成长的具体例子。

## 下一步

- [后台智能](https://mem.nowledge.co/zh/docs/advanced-features) -> 图谱如何自动生长
- [你的知识，你做主](https://mem.nowledge.co/zh/docs/use-cases/shared-memory) -> 自由切换工具，不丢失上下文
- [穿越时间搜索](https://mem.nowledge.co/zh/docs/use-cases/bi-temporal) -> 时间查询和演化链

---

# 运作原理

Nowledge Mem 背后的技术概念。适合想要深入理解系统行为的用户。

## 这个章节讲什么

这个章节解释 Nowledge Mem 在后台做了什么。写给那些不只想知道怎么用，还想知道为什么这么设计的用户。

你不需要读完这些页面才能用好 Mem。这里讲的一切都是自动运行的。但如果你好奇过为什么某些记忆在搜索中排得更靠前、系统怎么发现矛盾、或者你睡觉的时候它在干什么，答案都在这里。

- **[知识演化](https://mem.nowledge.co/zh/docs/concepts/evolves)**：你修正了一个决定，或者学到了和之前矛盾的东西。系统怎么追踪这种变化，同时保留完整历史。
- **[搜索架构](https://mem.nowledge.co/zh/docs/concepts/search-architecture)**：搜索不只看关键词。语义相似度、实体关联、社区聚类、标签、图遍历，六种策略并行，然后混合排序。
- **[记忆衰减](https://mem.nowledge.co/zh/docs/concepts/memory-decay)**：为什么有些记忆排得更靠前。最近用的、经常用的知识优先级更高，但重要的知识不会消失。
- **[后台智能](https://mem.nowledge.co/zh/docs/concepts/background-intelligence)**：你睡觉的时候系统在干什么。跑了哪些任务、什么时候跑、怎么防止它浪费资源。
- **[知识结晶](https://mem.nowledge.co/zh/docs/concepts/crystals)**：多个独立来源说了同一件事，系统把它们合成一份参考文档。

## 从哪里开始

如果你想搞清楚某个搜索结果为什么出现（或没出现），先看[搜索架构](https://mem.nowledge.co/zh/docs/concepts/search-architecture)和[记忆衰减](https://mem.nowledge.co/zh/docs/concepts/memory-decay)。

如果你想理解后台功能（简报、矛盾检测、实体提取），从[后台智能](https://mem.nowledge.co/zh/docs/concepts/background-intelligence)开始。

---

# 知识演化

Nowledge Mem 如何追踪你的认知变化，同时保留完整历史。

认知是会变的。三月做了一个决定，六月调整了一下，十月推翻了。大多数系统要么直接覆盖旧版本，要么把所有东西堆在一个平铺列表里。当你需要理解自己的思路是怎么演变的，这两种方式都不好用。

Nowledge Mem 用一个叫 **EVOLVES** 的模型，把相关的记忆用明确的关系链接起来。历史不会丢，重复也不会淹没你。你看到的是知识怎么一步步变成现在这样的。

## 四种关系类型

当系统检测到一条新记忆与已有记忆相关时，会创建以下四种链接之一：

| 关系 | 含义 | 典型场景 |
|------|------|----------|
| 替代（Replaces） | 你的认知发生了变化，新记忆取代旧记忆。 | "用 CockroachDB"替代"用 PostgreSQL" |
| 丰富（Enriches） | 你给已有知识增加了深度或细节。 | "React 19 增加了编译器"丰富了"React 18 引入了并发渲染" |
| 印证（Confirms） | 另一个独立来源认同了已有记忆。 | 两次独立的代码审查都推荐了同一个库 |
| 质疑（Challenges） | 新信息与之前记录的内容相矛盾。 | 三月的评估与十月的结论不一致 |

这四种类型覆盖了知识真正变化的方式：被更新、被扩展、被验证、被质疑。

## 实际效果

搜索一个话题时，EVOLVES 链会和单条结果一起呈现。你看到的不只是最新版本，而是你怎么走到这一步的完整路径。

对决策类知识尤其有用。搜索"数据库选型"不会只返回最近的决定，而是整条链：最初的选择、补充了理由的丰富、提出顾虑的质疑、最终定论的替代。

## 不需要版本控制的版本追踪

EVOLVES 链接区分了两类关系：
- **演进**（替代、丰富）构成版本链。旧版本被标记为已过时，搜索优先展示最新认知。
- **验证**（印证、质疑）是证据，不是版本。印证或质疑一条记忆不会替代它，两边都保持活跃。

标签会沿着演进链自动传播。如果你给一条记忆打了"架构"标签，后来一条新记忆替代了它，新记忆会继承这个标签。

## 检测机制

开启后台智能后，系统会将新记忆与你的已有知识进行比对。如果语义相似度足够高，就会评估关系类型并创建对应的链接。

这个过程在后台自动运行，通常在保存新记忆后一分钟内完成。你也可以在任何记忆的详情页直接查看和管理 EVOLVES 链接。

## 矛盾的处理

矛盾（质疑关系）会被呈现出来，而不是自动解决。系统把两条记忆并排展示，让你来决定：保留新的、两个都保留、或者忽略这个质疑。

这是有意为之。让系统替你判断哪个版本是"对的"，不是我们想做的事。

## 延伸阅读

- [后台智能](https://mem.nowledge.co/zh/docs/concepts/background-intelligence)：解释 EVOLVES 检测什么时候运行，以及其他后台任务如何使用它
- [知识结晶](https://mem.nowledge.co/zh/docs/concepts/crystals)：解释当多条 EVOLVES 链汇聚时会发生什么
- [搜索架构](https://mem.nowledge.co/zh/docs/concepts/search-architecture)：讲解 EVOLVES 链如何影响搜索排序

---

# 搜索架构

Nowledge Mem 如何组合六种搜索策略来找到对的记忆，即使用词完全不同。

搜索个人知识和搜索网页是两回事。你在找的是自己存下来的东西，用的是当时自己的措辞，可能已经过了好几个月。最大的挑战不是速度，而是你现在的提问方式和当初的记录方式之间的差距。

Nowledge Mem 通过同时从多个角度搜索来解决这个问题，然后混合排序。

## 六种搜索策略

每次搜索查询最多并行跑六种策略：

1. **语义搜索** - 用向量嵌入比较查询和每条记忆的含义。"API 认证"能匹配到"JWT token 配置"，即使用词完全不同。大多数查询靠这个。

2. **全文检索** - 做关键词匹配，支持中日韩分词。你记得确切的术语、函数名或代码标识符时，这个最管用。

3. **实体搜索** - 通过知识图谱找关联。搜"数据库性能"可以找到关于"PostgreSQL 索引"的记忆，因为两者共享实体关联，即使原文从没提过"数据库性能"。

4. **社区搜索** - 利用图谱中的社区聚类（强连接节点的集群）。有些记忆靠关键词或语义匹配找不到，但在同一个社区里就能找到。

5. **标签搜索** - 让你自己打的标签参与排序。查询命中某个标签时，带这个标签的记忆相关性会提高。

6. **图遍历** - 沿关系边（EVOLVES 链、实体关联、结晶来源）查找，靠的是图结构连接而非文本相似度。

## 快速模式和深度模式

不是每次搜索都需要全部六种策略全力运行。

**快速模式**（大多数查询 100ms 以内）并行跑语义搜索、全文检索和实体匹配。大约 90% 的查询用这个就够了，也是默认行为。

**深度模式**在快速结果基础上加一层 LLM 分析。先分类查询意图（查事实？探索概念？追踪关系？），再调整策略权重。需要的话还会用 HyDE（假设文档嵌入）来弥合表达差距，或者让 LLM 重新评估头部结果来改善排序。

查询带时间意图（"我上个季度决定了什么？"）或者快速模式结果置信度不高时，深度模式自动触发。也可以手动开。

## 结果排序

最终排序混合了上述策略的信号与记忆级别的评分：

- **语义相关性**是主导因素。语义上接近查询的记忆排得高，其他信号影响有限。
- **衰减分数**给最近和频繁访问的记忆适度加分。详见[记忆衰减](https://mem.nowledge.co/zh/docs/concepts/memory-decay)。
- **置信分数**给证据充分的记忆（多次访问、EVOLVES 链接、结晶引用）小幅额外加分。
- **知识结晶**因为是经过验证的合成知识，有排序加分。EVOLVES 链中的最新版本也比被取代的旧版排得更高。

每条结果都附带 `source_thread_id`（如果有的话）。如果智能体或工具需要记忆本身之外的更多上下文，可以拉取原始对话。

## 时间搜索

带有时间引用的查询（"2020 年发生了什么？"、"上个季度的决定"）会激活时间匹配。系统为每条记忆追踪两个独立的时间戳：
- **事件时间**：事情发生的时间
- **记录时间**：你保存它的时间

时间匹配会增加相关性提升，但不会覆盖语义相关性。来自正确时间段的记忆仍然需要语义上相关才能排名靠前。时间是信号，不是过滤器。

日期精度会被明确追踪。如果一条记忆的事件时间只精确到年，系统不会假装知道具体月份。

## 延伸阅读

- [记忆衰减](https://mem.nowledge.co/zh/docs/concepts/memory-decay)：解释影响排序的衰减分数和置信分数
- [知识演化](https://mem.nowledge.co/zh/docs/concepts/evolves)：讲解 EVOLVES 链如何影响搜索结果中的呈现
- [搜索与相关性](https://mem.nowledge.co/zh/docs/search-relevance)：包含实用搜索技巧的参考页面

---

# 记忆衰减

Nowledge Mem 如何决定优先展示什么，同时不让任何东西真正消失。

你记得昨天的会议比上个月的更清楚。你每天都用的事实比只看过一次的更容易回想起来。人类记忆之所以这样运作，是因为不得不如此。无限回忆但没有优先级，和忘掉一切一样没用。

Nowledge Mem 应用了类似的思路。每条记忆都有两个独立的分数，影响它在搜索结果中的位置。一个随时间衰减，另一个只增不减。

## 两个分数，两个职责

**衰减分数**反映新鲜度。今天存的记忆比半年前存的分数更高（其他条件相同）。分数按指数曲线随时间下降，但你经常访问的记忆因为频率加成会保持较高分数。

**置信分数**反映支撑程度。从一个基线开始，随证据积累而增长，永远不会下降。被多次访问、通过 EVOLVES 链关联、或者被用作知识结晶来源的记忆，置信度比孤立的、没碰过的记忆高。

两个分数互相独立。一条记忆可以很旧（低衰减）但有充分支撑（高置信），或者很新（高衰减）但未经验证（低置信）。两个信号都会和语义相关性一起，参与最终的搜索排序。

## 各个信号的来源

### 衰减分数

两个分量混合而成：
- **时近性**：从上次交互到现在的指数衰减。越近分数越高。半衰期按周算，不是按小时，所以记忆不会一夜消失。
- **频率**：访问次数的对数函数。经常用的记忆保持新鲜，哪怕最近几天没碰。

时近性的权重高于频率。一条去年大量使用但此后没碰过的记忆仍然会衰减，只是比从未访问过的慢一些。

### 置信分数

六种信号，每种都有上限以防单一信号主导：
- 访问频率：被检索了多少次
- 搜索出现次数：在搜索结果里露面的频率
- 主动点击：你打开它读了多少回
- 阅读时长：每次打开停留了多久
- EVOLVES 边：有多少其他记忆印证或丰富了它
- 结晶归属：有没有被用作知识结晶的来源

这些信号都是自动追踪的。你不需要手动评分或标记，系统就能学会哪些记忆重要。

## 重要性地板

衰减有个问题：一条真正重要的事实，几个月没访问，分数就会很低。它依然正确、有价值，但排不上来了。间隔重复系统靠安排复习解决这个，不过 Mem 不是闪卡应用。

所以每条记忆有一个**重要性地板**。衰减分数可以降，但不会低于一个和重要性等级挂钩的最低值。基础性的决定、关键操作流程，哪怕你很久没看，也始终能搜到。

地板是温和的。不会压过语义相关性，只是防止"幽灵记忆"：知识明明在系统里，但搜不到。

## 衰减如何影响搜索排序

语义相关性是搜索结果中的主导因素。衰减和置信是次要信号，在语义分数接近时调整排序。

实际效果：
- 两条与查询同样相关的记忆 -> 更新的那条排得更高
- 语义匹配度强的结果总是胜过最近但弱相关的
- 支撑充分的记忆（高置信）获得小幅额外提升
- 知识结晶和 EVOLVES 链中的最新版本在此基础上还有各自的排序调整

## 自动刷新

系统每天作为后台任务重新计算衰减和置信分数。刷新同时更新搜索中使用的缓存分数，所以结果能反映当前的使用模式，无需手动干预。

你还可以选择开启自动归档。跌破很低衰减阈值、从未被访问、且足够老的记忆可以被移入归档状态。自动归档默认关闭，需要明确开启。归档的记忆不会被删除，随时可以恢复。

## 延伸阅读

- [搜索架构](https://mem.nowledge.co/zh/docs/concepts/search-architecture)：解释衰减和置信如何参与完整的排序管线
- [知识演化](https://mem.nowledge.co/zh/docs/concepts/evolves)：讲解 EVOLVES 链，它是置信分数的信号之一
- [知识结晶](https://mem.nowledge.co/zh/docs/concepts/crystals)：解释结晶归属，另一个置信信号

---

# 后台智能

Nowledge Mem 在你不用它的时候做了什么，以及防止它浪费资源的保护机制。

Mem 里大部分有用的工作都在后台完成。实体提取、EVOLVES 检测、矛盾标记、知识结晶合成、工作记忆更新，都不需要你动手。你存一条记忆或导入一段对话，剩下的系统来。

这个页面讲跑了什么、什么时候跑、怎么防止失控。

## 两类触发方式

后台任务按触发方式分为两类。

### 定时任务

按固定时间运行，和你今天做了什么无关。

- **每日简报**，每天清晨。回顾近期活动，生成洞察，标记矛盾，为每个活跃分区写一份新的 Working Memory。Default 分区保留兼容文件 `~/ai-now/memory.md`。
- **结晶审查**，每周一次。找出可以合成为知识结晶的相关记忆集群。
- **洞察检测**，每周一次。在知识库里搜索跨领域关联和模式。
- **记忆压缩**，每周一次。识别可以整合的低价值集群。
- **社区检测**，定期运行。重建实体图谱的社区结构，供搜索使用。
- **衰减刷新**，每天运行。重新计算所有记忆的衰减和置信分数。

### 事件驱动任务

这些因为你做了某件事而触发，有一个短暂的延迟。

- **EVOLVES 检测**：保存新记忆时触发。检查新记忆是替代、丰富、印证还是质疑了已有知识。详见[知识演化](https://mem.nowledge.co/zh/docs/concepts/evolves)。
- **实体提取**：同步触发。为知识图谱提取实体和关系。
- **工作记忆刷新**：新记忆到达时触发。更新当前分区的 Working Memory，让连接的智能体尽快看到新上下文。
- **集群评估**：EVOLVES 边创建后触发。看新集群是否达到了结晶的形成条件。

## 级联

这些任务不是独立的，一个动作可以触发一整条链。

你存一条记忆 -> EVOLVES 检测跑起来 -> 发现和一条旧记忆的"印证"关系 -> 集群评估触发 -> 发现三条相关记忆构成了足够强的集群 -> 知识结晶被创建。

每一步都有自己的延迟窗口，系统是攒一批再处理。连续存五条记忆，系统会放在一起分析，不会跑五遍。

## 四层保护

后台智能消耗 LLM token。没有限制的话，一波密集操作可能耗尽 token 预算，或者赶工太多产出低质量结果。四层机制防住这个。

1. **防抖**：事件驱动任务执行前等一段时间。等待期间又来了同类事件，计时器重置。导入一个长对话时，系统不会每条消息都分析一遍，而是合并成一次。
2. **频率限制**：每小时能跑的 LLM 任务有上限。超了就延迟，不丢弃。
3. **Token 预算**：可以设每小时和每天的 token 上限。预算花完后 LLM 任务暂停到下一个周期。不用 LLM 的任务（衰减刷新、社区检测）不受影响。
4. **质量门控**：抑制低价值输出。每日简报如果产出零洞察、零结晶、零标记，就保持沉默，不会生成一张"无事可报"的卡片。洞察检测会比对过去两周的记录避免重复。结晶至少需要三个汇聚来源。

## 上下文注入

每个后台任务启动前会收到预算好的上下文。每日简报拿到的是过去一周的活动摘要、昨天的工作记忆、图谱统计和最近的矛盾解决记录。省掉 LLM 自己探索的步骤，直接聚焦。

上下文有大小上限，超了就先裁低优先级的部分。

## 工作记忆

工作记忆是每日简报最直接的产出。每天早上归档昨天的，基于近期活动写一份新的。

Default 分区保留文件 `~/ai-now/memory.md`。如果你使用 spaces，其他分区也会通过同样的 Mem 接口拥有各自的 Working Memory。Claude Code、Cursor、Codex 这些工具之所以知道你最近在做什么、做了什么决定，靠的就是这份简报。

白天也会更新。存了新记忆后系统会刷新它，延迟比其他事件驱动任务长一些（因为跑一次成本更高）。你也可以手动编辑。

## 设置

每个后台任务都有独立的开关。你可以关掉 EVOLVES 检测但保留每日简报，或者禁用洞察检测但保留实体提取。总开关可以一次性关闭全部。

Token 预算和调度参数（简报时间、社区检测间隔）也可以配置。后台智能需要配置远程 LLM，因为任务在你的机器上运行，需要一个模型来推理。

## 延伸阅读

- [记忆衰减](https://mem.nowledge.co/zh/docs/concepts/memory-decay)：讲解衰减刷新任务以及分数如何计算
- [知识演化](https://mem.nowledge.co/zh/docs/concepts/evolves)：详细解释 EVOLVES 检测
- [知识结晶](https://mem.nowledge.co/zh/docs/concepts/crystals)：解释结晶审查和什么触发结晶创建



---

# 第七部分：部署与参考

# Linux 服务器部署

> 来源: https://mem.nowledge.co/zh/docs/server-deployment

Nowledge Mem 可以在没有图形界面的 Linux 服务器上以无头模式运行。真正的服务器部署，建议直接使用 Linux 安装包，然后通过命令行管理；之后你可以在同一台机器的浏览器里打开内置 Web App，也可以用 API key 让其他客户端连接。
可用性每日简报、洞察检测、知识图谱丰富等后台智能能力，需要已配置的远程 LLM，以及你的当前版本所对应的许可能力。这个页面关注的是部署方式，而不是套餐包装。
sudo只用于安装软件包和安装 system 服务。
平时运行nmem serve、nmem config ...、nmem license ...、nmem tui这类命令时，请使用你的普通 Linux 用户。
新版本会默认阻止你通过sudo或其他提权方式去运行这些会写本地状态的命令，避免 fresh install 之后在错误的用户环境里悄悄留下root属主的 Mem 文件。
如果你本来就是直接登录root、并且希望把 Mem 放在/root下运行，这种场景仍然支持。

## 系统要求

要求规格操作系统Ubuntu 22.04+、Debian 12+，或兼容版本（通过 AppImage）架构x86_64内存 (RAM)最低 8 GiB（推荐 16 GiB）磁盘空间10 GiB 可用空间依赖libgtk-3-0、libwebkit2gtk-4.1-0、zstd（.deb自动安装）

## 安装

APT 仓库（推荐）手动安装 DEBAppImage（便携兜底）设置 APT 仓库以通过apt upgrade自动更新：curl-fsSLhttps://nowledge-co.github.io/community/apt/install.sh|sudobashsudoapt-getinstallnowledge-mem添加 GPG 签名密钥和仓库源。之后如果只想更新 Mem，可运行sudo apt-get update && sudo apt-get install --only-upgrade nowledge-mem。常规整机升级仍可使用sudo apt-get update && sudo apt-get upgrade，或通过unattended-upgrades（如已配置）自动应用。BROWSER_UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36'# 下载 .debcurl-A"$BROWSER_UA"-L-onowledge-mem.debhttps://nowled.ge/download-mem-deb# 安装包sudodpkg-inowledge-mem.deb# 修复缺失的依赖sudoapt-getinstall-f-yBROWSER_UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36'# 下载 AppImagecurl-A"$BROWSER_UA"-L-onowledge-mem.AppImagehttps://nowled.ge/download-mem-appimage# 仅适合便携式手动运行chmod+xnowledge-mem.AppImage./nowledge-mem.AppImage
如果你要的是一台真正的无头 Linux 服务器，请优先用 APT 或.deb。
AppImage 更适合便携式、手动运行，不适合作为长期的 server + systemd 主路径。
它不会把nmemCLI 装进 PATH，所以在没有桌面会话的机器上，也不是最顺手的nmem工作流。
如果你是在终端里下载安装包，请直接照着上面的命令执行。
这些命令已经带上了浏览器 User-Agent，因为有些下载链接会拦截普通的curl/wget，直接返回403。
APT 和.deb安装后会自动完成以下操作：
- 解压内置的 Python 运行时
- 在/usr/local/bin/nmem创建nmemCLI
- 配置 APT 仓库以自动更新（通过 APT 安装时）
- 设置桌面启动项（在无头服务器上可忽略）
验证 CLI 可用：

```
nmem --version
```

如果你用的是 AppImage，那么之后每次都需要直接运行 AppImage 文件。它不会把nmem命令加到 PATH 里。

## 快速开始

如果你是要搭一台真正的 Linux 服务器，先把后台服务装好。这样后面的许可证、模型、配置命令都能在新的 SSH 会话里直接用，也不怕重启以后丢掉。
先安装后台服务服务器场景推荐：sudonmemserviceinstall--service-user<linux-user>如果你确实要用用户级服务：nmemserviceinstall--user无头服务器上，优先用 system 服务。
用户级服务只有在该账号开启 lingering 之后，才会在退出登录和重启后继续存在：sudologinctlenable-linger<linux-user>确认服务已经起来nmemservicestatusnmemstatus激活许可证nmemlicenseactivate<许可证密钥># 通常会自动从密钥中识别邮箱nmemlicensestatus# 验证激活状态nmemlicenserenew# 以后如果授权过期，可刷新这台设备的授权如果自动识别失败，再显式运行nmem license activate <许可证密钥> <邮箱>。打开 Web App先把浏览器登录信息打印出来：nmemkey--show-login如果你当前这个版本还不支持--show-login，就先运行：nmemkey然后打开它打印出来的地址。默认情况下通常是：http://127.0.0.1:14242/app无头服务器上，浏览器通常在另一台电脑上。请直接使用nmem key --show-login打印出来的那个地址和端口。如果需要 SSH 转发，也转发同一个端口：ssh-L<port>:127.0.0.1:<port><server>然后在你自己的浏览器里打开对应的本地地址，再粘贴 API key。下载搜索索引模型nmemmodelsdownloadnmemmodelsstatus# 验证安装下载用于混合搜索的索引模型（约 500 MB），只需下载一次。如果你是在无头服务器上使用，建议把下面两个命令当作搜索健康检查入口：nmem status：看搜索现在是已就绪、需要重建，还是只是在后台补齐元数据nmem models status：同时查看模型状态，以及当前搜索索引是否需要你介入配置 LLM 提供商Linux 上需要远程 LLM（不支持本地 LLM）：nmemconfigprovidersetanthropic\--api-keysk-ant-xxx\--modelclaude-sonnet-4-20250514nmemconfigprovidertest# 验证连接支持的提供商包括：anthropic、openai、gemini、xai、deepseek、minimax、zai、moonshot、ollama、openrouter以及 OpenAI 兼容端点。完整提供商矩阵与注意事项见：LLM 提供商。启用后台智能nmemconfigsettingssetbackgroundIntelligencetruenmemconfigsettingssetautoDailyBriefingtrue验证所有配置nmemstatus如果你暂时不想装 systemd 服务也可以先直接前台跑：nmemserve但要记住：这个终端会一直被占着。后面的nmem license ...、nmem models ...、nmem config ...需要在第二个终端里执行。

## 作为 systemd 服务运行

生产部署建议使用nmem service install设置后台 systemd 服务，开机自动启动：
系统服务 (root)用户服务 (无需 root)# 以你的日常 Linux 用户身份安装、启用并启动sudonmemserviceinstall--service-user<linux-user># 如果你本来就是从那个 Linux 用户直接 sudo 上来的，Mem 通常也能自动识别sudonmemserviceinstall# 自定义主机/端口sudonmemserviceinstall--service-user<linux-user>--host0.0.0.0--port8080# 无需 root 权限nmemserviceinstall--user
system 服务应当以你的日常 Linux 用户运行，而不是root。
如果sudo不能正确识别目标账号，请加上--service-user <linux-user>。
如果你用的是用户级服务，请直接以该用户运行nmem service install --user，不要加sudo。
如果你希望用户级服务在退出登录和重启后继续存在，还需要额外执行一次sudo loginctl enable-linger <linux-user>。
如果你的 VPS 本来就是只用root，并且你也是直接登录root，那么nmem service install仍然可以一致地使用/root。
如果你之前在旧版本里用root运行过，~/.config/co.nowledge.mem.desktop/下面有些文件可能仍然属于root。
这时许可证操作、LLM 提供商设置保存，或者 Access Anywhere 设置都可能失败，先把这些文件的属主改回运行服务的 Linux 用户。
新版本在发生这种情况时会直接告诉你是哪一个路径有问题。
常见修复命令如下：sudochown-R<linux-user>:<linux-user>~/.config/co.nowledge.mem.desktopsudochown-R<linux-user>:<linux-user>~/.local/share/NowledgeGraph如果你自定义了XDG_CONFIG_HOME或XDG_DATA_HOME，请改成对应目录。

### 管理服务


```
nmem service status           # 查看服务状态
nmem service logs -f          # 跟踪服务日志
nmem service stop             # 停止服务
nmem service start            # 启动服务
nmem service uninstall        # 停止、禁用并删除服务
```

如果安装的是用户级服务，请在任何nmem service命令后添加--user。

### serve 与 service 的区别

nmem servenmem service install运行方式前台（当前终端）后台（systemd）何时停止Ctrl+C 或关闭终端nmem service stop或系统关机开机自启否是（自动启用）适用场景测试、开发生产部署

## 数据位置

所有数据（图数据库、搜索索引、版本元数据）存储在一个目录中：

```
~/.local/share/NowledgeGraph/
├── nowledge_graph_v2.db/   # KuzuDB 图数据库
├── search_index/           # LanceDB 向量 + BM25 索引
└── db_version.json         # Schema 版本追踪
```

服务器使用XDG_DATA_HOME（默认为~/.local/share）自动解析此路径。如需使用自定义位置，请在启动服务器之前设置NOWLEDGE_DB_PATH环境变量：

```
export NOWLEDGE_DB_PATH=/mnt/data/NowledgeGraph/nowledge_graph_v2.db
nmem serve
```

从旧版本升级旧版本将数据存储在运行nmem serve的工作目录下的./data/文件夹中。升级后，服务器会自动检测旧数据并打印迁移说明。如果数据似乎丢失，请参阅下方故障排除：升级后数据丢失。

## 远程访问

默认情况下，nmem serve只监听127.0.0.1。这已经足够支持：
- 服务器本机上的nmem命令
- 本机浏览器访问同一个本地地址（默认是http://127.0.0.1:14242/app）
如果你把服务绑定到0.0.0.0或其他非 loopback 地址，Mem 就会要求其他设备带 API key 才能连接。需要时会自动生成 key，你之后也可以随时重新打印：

```
nmem key
# 或
nmem key --show-login
```

当服务器已经可访问之后，客户端应优先走自己支持的最高层入口：先用原生集成，其次是插件设置界面或nmem配置，只有没有更好专属路径时才直接配置 MCP。

```
# 在服务器上
nmem serve
nmem key
```

在远程机器上安装独立 CLI：

```
pip install nmem-cli
# 或
uv pip install nmem-cli
```

然后配置连接方式。推荐的持久化方式是：

```
nmem config client set url http://你的服务器:14242
nmem config client set api-key nmem_...
```

这会为当前机器写入~/.nowledge-mem/config.json。如果你只想在当前终端会话里临时覆盖，再使用环境变量：

```
临时终端覆盖export NMEM_API_URL=http://你的服务器:14242
export NMEM_API_KEY="nmem_..."
```


```
nmem status
nmem m search "查询内容"
```

优先级：CLI 参数 > 环境变量 > 配置文件 > 默认值。
真实会话保存的重要说明像nmem t save --from claude-code、gemini-cli、codex这样的真实会话保存，仍然会在运行该智能体的客户端机器上读取本机会话文件。把nmem指向远程服务器，只会改变规范化数据上传到哪里，不会把会话发现工作转移到服务器端。
如需关闭鉴权（不建议在生产环境使用）：

```
nmem serve --no-auth
```

安全提示默认情况下，其他机器连进来时都需要 API key。你如果启用了 localhost auth，本机浏览器访问/app也会要求输入 key。更严格的部署场景，建议再配合防火墙规则，或者直接使用随处访问 Mem的 Cloudflare tunnel。

## 交互式 TUI

使用 TUI 获得交互式终端体验：

```
nmem tui
```

TUI 提供完整的设置管理界面，包括许可证激活、LLM 配置和知识处理开关。
现在你也可以在Settings中直接配置Access Anywhere（快速链接 / Cloudflare 账号稳定链接），而且同一个界面也能把本地 Web 登录地址和当前 API key 展示出来，所以即使在纯终端环境里，也能把/app的登录流程走通。完整指南见：随处访问 Mem。
如果服务器网络会拦截 UDP/QUIC，导致 Access Anywhere 启动不上来，可以在重启 Mem 前先强制 Cloudflare 使用 HTTP/2：

```
export TUNNEL_TRANSPORT_PROTOCOL=http2
```


## 配置参考


### 环境变量

变量默认值描述NMEM_API_URLhttp://127.0.0.1:14242CLI 命令的服务器地址NMEM_API_KEY-用于鉴权的 API keyNOWLEDGE_DB_PATH自动检测覆盖数据库位置NOWLEDGE_BACKEND_HOST127.0.0.1服务器绑定地址NMEM_LAN_AUTH-设为disabled可跳过鉴权（等同于--no-auth）

### CLI 命令摘要

命令描述nmem serve启动服务器（默认只监听本机）nmem serve --no-auth启动服务器但不要求 API key 鉴权nmem service install安装并启动 systemd 服务nmem service status查看 systemd 服务状态nmem service logs -f跟踪服务日志nmem service stop/start停止或启动服务nmem service uninstall删除 systemd 服务nmem status检查服务器状态nmem license activate <key> [email]激活许可证（通常会自动识别邮箱）nmem license renew刷新或续期这台设备的授权nmem models download下载索引模型nmem config provider set <p> --api-key <k>配置 LLM 提供商nmem config provider test测试 LLM 连接nmem config settings显示处理设置nmem config settings set <key> <value>更新设置nmem update检查可用更新nmem update apply下载并应用更新nmem tui交互式终端 UInmem key打印当前 API keynmem key --show-login同时打印本地 Web App 地址和 API key

## 故障排除


### 升级后数据丢失

旧版本（0.7 之前）将数据库存储在运行nmem serve的工作目录下的./data/文件夹中。当前版本将数据存储在标准位置（~/.local/share/NowledgeGraph/）。如果升级后记忆消失了，你的数据很可能仍在磁盘上，只是在旧位置。
查找旧数据搜索旧数据库可能存在的常见位置：find/-name"nowledge_graph*.db"-typed2>/dev/null常见位置包括：~/data/nowledge_graph.db，如果从主目录运行nmem serve/data/nowledge_graph.db，如果 systemd 系统服务在/目录下运行（未设置WorkingDirectory）/你运行nmem的路径/data/nowledge_graph.db，其他工作目录移动到标准位置# 先停止服务器nmemservicestop# 如果使用 systemd# 或按 Ctrl+C            # 如果在前台运行# 如需创建标准目录mkdir-p~/.local/share/NowledgeGraph# 移动数据库和相关文件mv/旧路径/data/nowledge_graph*.db*~/.local/share/NowledgeGraph/mv/旧路径/data/search_index~/.local/share/NowledgeGraph/2>/dev/nullmv/旧路径/data/db_version.json~/.local/share/NowledgeGraph/2>/dev/null# 重启服务器nmemserve# 或: nmem service start验证恢复nmemmsearch"测试"# 搜索你的记忆nmemstatus# 检查服务器状态从当前版本开始，nmem serve会在启动时打印数据库路径，方便确认使用的存储位置。如果在./data/下检测到旧数据，服务器会自动打印迁移说明。

## 下一步

- CLI 参考- 完整的 CLI 文档
- API 参考- REST API 端点
- 集成- 连接 AI 工具

---

# LLM 提供商

> 来源: https://mem.nowledge.co/zh/docs/llm-providers

选择远程 LLM 时，可以分成三层来理解：
- 套餐或订阅：你已经在用、也已经付费的账号体系
- 提供商或端点：请求最终发到哪里
- 模型：你日常真正运行的具体模型
在设置中（或nmemCLI/TUI）配置一次之后，就可以长期使用。
Nowledge Mem 当前推荐优先使用订阅型默认路径，日常体验更稳定：
- OpenAI ChatGPT/Codex 订阅
- Kimi Code 订阅
成本优先的默认建议日常 AI Now 使用，建议默认优先选择支持工具调用且更快的模型，不必一开始就用 SOTA。例如：gpt-5.1-codex-mini（Codex 订阅）或 Kimi Coding Plan 相关模型。如果你现在 token 消耗偏高（例如长期固定使用gpt-5.3-codex），优先切换到更轻量、支持 tool use 的默认模型。

## 如何选择

OpenAIChatGPT/Codex 订阅代码与通用任务都稳定，适合作为默认选择。MoonshotAIKimi Code 订阅适合编程工作流，AI Now 工具调用表现稳定。

## 提供商章节指南

OpenAIOpenAI（ChatGPT/Codex）适合：编程 + 日常助手主力场景。AI Now 与智能体工具调用稳定生态兼容性强，默认选择更省心如果你用的是 ChatGPT Subscription，这里请选择 Codex 模型或基础 GPT-5 版本，不要选gpt-5-chat-latest这类聊天别名。MoonshotAIKimi / Moonshot适合：Kimi Code 订阅与编码工作流。AI Now 工具调用体验稳定日常编码使用顺畅ClaudeAnthropic Claude适合：看重自主工作流稳定性的用户。工具调用与规划质量可靠适合长链路、多步骤任务DeepSeekDeepSeek适合：关注成本/性能平衡的场景。AI Now 建议使用deepseek-chat在保留工具工作流时具备较好性价比OpenRouterOpenRouter适合：一个端点接入多模型。多模型路由灵活AI Now 场景请选择支持工具调用的模型GeminiGemini适合：已在 Google 生态中的用户。AI Now 与智能体场景可用已有 Google AI Studio 凭据时接入顺滑GrokxAI适合：Grok 用户。支持 AI Now 工具工作流适合已在 xAI 生态中的团队MinimaxMiniMax适合：已在 MiniMax 生态中的用户。AI Now 与扩展工作流均支持兼容当前可用的 MiniMax 聊天模型Z.aiZ.AI适合：智谱生态用户。AI Now 与智能体流程支持区域与生态匹配度友好OllamaOllama适合：本地优先 / 自托管用户。模型运行不依赖公有云AI Now 建议选择支持工具调用的模型GithubCopilotGitHub Copilot适合：已有 Copilot 订阅体系的团队。AI Now 支持对既有 Copilot 用户迁移成本低APIOpenAI 兼容自定义端点适合：私有网关、自托管、企业代理。端点需实现 OpenAI 兼容 chat completions工具调用能力取决于网关与模型本身
DeepSeek 快速提示DeepSeek 在 AI Now 中请选择deepseek-chat。

## 上下文窗口

每个模型对单次请求可处理的 token 数有上限。Nowledge Mem 会根据模型名称自动检测这一限制——例如gpt-4o默认 128k，gemini-2.0-flash默认 1M。
你可以在设置 → 服务商 → 高级选项中手动覆盖上下文窗口大小（也可通过nmem config provider set --context-window <tokens>命令行设置）。
什么时候需要调整：
- 小型或微调模型（8k–32k 上下文）——设置实际限制，让 AI Now 在溢出前自动压缩对话。
- 超大上下文模型（500k–1M+）——增大窗口，让 AI Now 充分利用模型容量，避免过早压缩。
- 自定义或自托管模型——如果模型名称不匹配已知模式，默认为 128k。设置真实值以获得准确的压缩行为。
对话压缩机制当对话接近上下文上限时，AI Now 会自动总结较早的消息并保留最近的交互。这让长对话可以持续进行而不丢失重要上下文。正确设置上下文窗口，可以确保压缩在恰当的时机触发——不会太早，也不会太晚。

## 自定义提供商建议

如果你使用 OpenAI 兼容自定义端点（openai_compatible）并指向 DeepSeek（api.deepseek.com），在 AI Now 与智能体场景中请将模型配置为deepseek-chat。
自定义端点还支持新版Responses API（/v1/responses），与传统的 Chat Completions 格式并存。添加或编辑服务商时可选择 API 格式。
Linux 无头部署配置请参考：Linux 服务器部署。

---

# 搜索与相关性

> 来源: https://mem.nowledge.co/zh/docs/search-relevance

搜索由多信号评分、时间衰减和反馈循环驱动。下面逐一说明。

## 评分管道

搜索时，Nowledge Mem 不只匹配关键词，而是综合多个信号来排列结果。

### 语义评分

此轨道查找与你要找的内容匹配的记忆：
- 基于含义的搜索：按语义相似性查找记忆，而不仅仅是精确词语。搜索"设计模式"并找到关于"架构方法"的记忆。
- 关键词搜索：使用 BM25 排序捕获精确短语和技术术语。
- 标签匹配：浮现带有匹配标签的记忆。
- 图遍历：通过实体和主题社区发现连接的记忆。

### 衰减、置信度与时间评分

此轨道根据新鲜度、验证度和你的使用情况调整结果：
- 时效性：最近访问的记忆得分更高。我们使用大约30天半衰期的指数衰减。
- 频率：你反复访问的记忆变得更加持久（对数缩放，收益递减）。
- 重要性底线：高重要性记忆即使未使用也保持最低可访问性。
- 置信度：通过使用和知识图谱连接验证过的记忆获得微妙提升。置信度随证据增长，永不下降。
- 时间匹配：提升事件时间与你查询匹配的记忆（仅深度模式）。
这些轨道组合成决定结果排序的最终分数。

## 记忆衰减

记忆会随时间自然消退，使用即强化。

### 工作原理

时效性：昨天访问的记忆得分比三个月前的要高得多。30天的半衰期意味着如果没有访问，分数大约每月减半。
频率：第 10 次访问比第 100 次更重要。早期重复建立持久性，后续收益递减。
重要性底线：高重要性记忆永远不会完全衰减。即使长期未访问，也保持最低可达性，防止基础知识丢失。

### 搜索强化

自 v0.6.6 起从 v0.6.6 开始，每次搜索展示都会更新记忆的最后访问时间和访问次数，自动增强其新鲜度分数。
此前，只有明确的点击才会更新新鲜度。现在，出现在搜索结果中也算作轻度访问，防止活跃相关的记忆逐渐衰减。

### 置信度

自 v0.6.7 起置信度评分在每日新鲜度刷新时计算。
独立于衰减，每条记忆会建立一个置信度评分，反映其被验证的程度。置信度从基线开始，随着证据积累而增长：
- 搜索使用：展示次数、点击和阅读时间
- 知识图谱：被其他记忆确认或丰富的记忆，或被用作知识结晶的来源
与随时间衰减的新鲜度不同，置信度只会增长。一条经常被访问、点击并与其他知识关联的记忆，在语义匹配相同的情况下，得分比新创建的记忆更高。
影响是微妙的（置信度约占最终评分的 5%），但它为成熟的知识提供一致的优势。

### 这意味着什么

- 活跃的知识保持新鲜，包括出现在搜索结果中的记忆
- 旧记忆不会消失，它们只是在同样相关时排名更低
- 无论访问模式如何，重要知识都会持续存在
- 经过验证的记忆获得微妙的排名提升
- 系统自动从你的行为中学习

### 自动维护

自 v0.6.6 起这些功能为可选启用，位于设置 → 处理 → 高级。
为了保持衰减分数准确和记忆质量，Nowledge Mem 可以运行后台维护：
- 新鲜度刷新：每日任务重新计算所有记忆的衰减和置信度分数，使排序保持准确。启用后，每日简报包含记忆健康摘要。
- 记忆整理：每周任务识别相似或冗余记忆的聚类，可以合并、生成简洁摘要，或标记不确定的情况交由你审查。不会删除记忆，只创建关联或摘要。
- 自动归档：可选功能，将新鲜度极低、零参与度且超过 90 天的记忆移至归档状态。归档记忆仍出现在搜索中但排名更低。默认关闭。

## 时间理解

Nowledge Mem 理解两种时间。

### 事件时间 vs 记录时间

事件时间是事情实际发生的时间：
- "2020年的产品发布"
- "上季度的决定"
- "在我们迁移之前"
记录时间是你保存记忆的时间。你今天可能记录一条关于2020年事件的记忆。
这对于像"关于2020年事件的最近记忆"这样的查询很重要：你最近保存的东西（记录时间）关于2020年的事件（事件时间）。

### 时间意图检测

深度模式功能时间意图检测需要深度模式搜索。在快速模式下，时间引用仅按关键词匹配。
在深度模式下，系统解释时间引用：
查询理解"2023年的决定"事件时间：2023"最近的记忆"记录时间：最近"关于2020年的最近记忆"事件：2020，记录：最近"迁移之前"事件：在那个事件之前
模糊引用如"上季度"、"大约2020年"或"今年初"被转换为有意义的过滤器。

### 日期精度

当你保存关于"2020年初"的记忆时，系统：
1. 规范化为可搜索的日期（2020-01-01）
2. 跟踪精度级别（年、月或日）
3. 保留原始含义以实现准确匹配
这让"2020年的记忆"（年精度）与"2020年1月的记忆"（月精度）工作方式不同。

## 反馈循环

你的使用模式持续改进搜索相关性。

### 我们跟踪什么

信号捕获的内容展示次数记忆在结果中出现的频率点击当你打开记忆查看详情时停留时间你花多长时间阅读

### 如何改进搜索

- 高点击率表明记忆确实有用
- 长停留时间表明内容有价值
- 频繁展示但没有点击可能表明相关性下降
无需任何操作，正常使用即可。

### 自动标签

自 v0.6.7 起创建新记忆时自动分配标签。
当你创建记忆时，后台智能会阅读内容并分配 2–4 个描述性标签。它会优先复用已有标签，遵循统一的小写连字符命名约定（如machine-learning或project-alpha）。这意味着你的记忆从创建起就被组织好了，无需手动标记。

## 图驱动的发现

知识图谱通过实体连接扩展搜索范围。

### 记忆如何连接

每条记忆可以链接到：
- 实体：提到的人员、概念、技术、地点
- 其他记忆：通过共享实体或关系
- 社区：图分析检测到的主题集群

### 通过连接搜索

实体介导：即使标记不同，也能通过共享实体如 PostgreSQL 或索引找到关于"数据库优化"的记忆。
社区介导：关于"认证"的搜索可能会浮现你"安全实践"社区的记忆。
图扩展：从一条记忆开始，探索连接的知识。

## 搜索模式

所有界面都有两种模式可用：

### 快速模式

- 通常不到100毫秒响应
- 直接语义和关键词匹配
- 实体和社区搜索，无需语言模型分析
- 最适合快速查找

### 深度模式

- 完整的语言模型分析
- 时间意图检测（例如，"最近在做的；过去十年的社交活动"）
- 查询扩展以获得更好的召回率
- 上下文感知的策略加权
- 更适合探索性搜索
两种模式都适用于主搜索、全局启动器和 API。

## 结果透明度

每条结果都附带排序原因。

### 搜索查询详情

每次搜索后，你可以查看你的查询如何被解释的详细分析：
- 使用了哪些搜索策略
- 时间意图检测结果（在深度模式下）
- 查询扩展和实体提取

### 分数分解

悬停在任何结果的分数上查看它是如何计算的分解：
- 语义分数：内容与你的查询匹配程度
- 衰减分数：基于时效性和频率的新鲜度
- 置信度：通过使用和图连接的验证程度
- 时间提升：事件时间相关性（适用时）
- 图信号：实体和社区连接
这帮助你理解使用模式如何影响排序，以及某条记忆为什么会出现在特定查询中。

---

# Nowledge Mem CLI

> 来源: https://mem.nowledge.co/zh/docs/cli

ThenmemCLI gives you terminal access to your Nowledge Mem knowledge base. Search memories, browse threads, read and edit Working Memory, explore the knowledge graph, and view your activity feed, all from the shell.
If you also need browser automation, the desktop app bundles a second CLI calledbrowse-now. It works with the Nowledge Mem Exchange extension to let agents control the user's real browser for authenticated and interactive web tasks. That browser bridge is local-only and is not exposed through Access Anywhere.

## Installation


### Option 1: Standalone PyPI Package

Install on any machine. Works with a local or remote Nowledge Mem server:

```
pip install nmem-cli

# or with uv
uv pip install nmem-cli

# or run without installing
uvx --from nmem-cli nmem --help
```

Requirements:Python 3.11+, Nowledge Mem running locally or reachable remotely.
Remote AccessThe standalone package lets you reach your Nowledge Mem from servers, CI/CD pipelines, or remote workstations. SeeAccess Mem Anywhere. View onPyPI.

### Option 2: Bundled with Desktop App

macOSGo toSettings → Preferences → Developer Toolsand clickInstall CLI.Installs to/usr/local/bin/nmem.0:00--:--WindowsThe CLI is automatically available after app installation. Open anew terminal windowto usenmem.LinuxIncluded with deb/rpm packages. The binary is placed in/usr/local/bin/nmem.

## Quick Start


```
nmem status                              # Check connection
nmem m search "project notes"           # Search memories
nmem m search "project notes" --space work
nmem m add "Key insight" --title "Learning"
nmem wm                                  # Read today's Working Memory
nmem wm --space work                     # Read Working Memory for one space
nmem spaces                              # List known spaces
nmem f --days 1                          # Today's activity
nmem g expand <memory-id>               # Explore graph connections
nmem tui                                 # Interactive terminal UI
```


## Global Options

OptionDescription-j, --jsonMachine-readable JSON output--api-url <url>API URL (default:http://127.0.0.1:14242)--space <space>Run the command inside one named space when you want a non-default context-v, --versionShow version-h, --helpShow help
Aliases:m= memories ·t= threads ·wm= working-memory ·g= graph ·f= feed ·c= communities

## Memory Commands (nmem m)

Spaces are optionalIf you do not use spaces, ignore--spaceand keep working in the default space. When you do need separate project or agent contexts, the same commands work with--space "<space name>".

### List memories


```
nmem m                        # Recent 10 memories
nmem m -n 50                  # List 50
nmem m --importance 0.7       # Minimum importance filter
```


### Search


```
nmem m search "authentication patterns"
nmem m search "authentication patterns" --space work
nmem m search "API design" --importance 0.8
nmem m search "deploy" -l devops -l backend    # Filter by labels (AND)
nmem m search "sprint" --mode deep             # Graph + LLM-enhanced results
```

Bi-temporal search.Distinguishwhen something happenedfromwhen you saved it:

```
nmem m search "database decision" --event-from 2025-01 --event-to 2025-06
nmem m search "meeting notes" --recorded-from 2026-01-01
```

OptionDescription-nMax results-l, --labelFilter by label (repeatable)--importanceMinimum importance (0–1)--modenormal(default, fast) ordeep(graph + LLM-enhanced)--event-from/toWhen the facthappened(YYYY, YYYY-MM, or YYYY-MM-DD)--recorded-from/toWhen it wassavedto Nowledge Mem (YYYY-MM-DD)

### Add


```
nmem m add "We chose PostgreSQL for task events"
nmem m add "Prefer functional components in React" \
  --title "Frontend conventions" \
  --unit-type preference \
  --importance 0.8 \
  -l frontend -l react

# Record when something actually happened (bi-temporal)
nmem m add "Decided to sunset the legacy API" \
  --unit-type decision \
  --event-start 2025-11 \
  --when past
```

OptionDescription-t, --titleMemory title-i, --importanceImportance 0–1-l, --labelAdd label (repeatable)--unit-typefactpreferencedecisionplanprocedurelearningcontextevent--event-startWhen it happened (YYYY, YYYY-MM, YYYY-MM-DD)--event-endEnd of a time range--whenpastpresentfuturetimeless(default: timeless)

### Show


```
nmem m show <id>
nmem m show <id> --content-limit 500
```


### Update


```
nmem m update <id> --title "New title"
nmem m update <id> --importance 0.9
nmem m update <id> --content "Updated content"
```


### Move between spaces

Use this when you already saved memories in one space and want to move them into another one.

```
nmem m move <id> --space "Research Agent" --to-space "Archive"
nmem m move <id1> <id2> --space "Research Agent" --to-space "Archive" -f
nmem m move --all-in-space "Research Agent" --to-space "Archive" --dry-run
nmem m move --all-in-space "Research Agent" --to-space "Archive" -f
```

--spaceis the source space.--to-spaceis the destination. For whole-space moves, use--dry-runfirst if you want to preview the count before changing anything.

### Delete


```
nmem m delete <id>
nmem m delete <id> -f          # Skip confirmation
nmem m delete <id1> <id2>      # Multiple IDs
```

nmem m deleteis interactive by default. For scripts, agents, CI, or any non-interactive shell, use-fso the command does not block waiting for confirmation.

## Thread Commands (nmem t)


### List and search


```
nmem t                                  # Recent threads
nmem t list --source openclaw -n 20     # Recent OpenClaw threads only
nmem t list --offset 20 -n 20           # Next page of recent threads
nmem t search "architecture decisions"
```

Uselistwhen you want the newest threads in order. Usesearchwhen you want full-text matches inside thread content.

### Show


```
nmem t show <id>
nmem t show <id> -n 50                 # Show up to 50 messages
nmem t show <id> --content-limit 200
```


### Create


```
# From text
nmem t create -t "Quick note" -c "Remember to review the API changes"

# From a file
nmem t create -t "Meeting notes" -f notes.md

# With structured messages
nmem t create -t "Chat session" \
  -m '[{"role":"user","content":"Hello"},{"role":"assistant","content":"Hi!"}]'

# With a stable ID (idempotent, safe to re-run)
nmem t create -t "OpenClaw session" --id "openclaw-abc123-session"
```


### Append

Add messages to an existing thread. Safely idempotent: duplicate messages are filtered by content hash or external ID.

```
# Single message
nmem t append <id> -c "Follow-up note"

# Structured messages
nmem t append <id> \
  -m '[{"role":"user","content":"Question"},{"role":"assistant","content":"Answer"}]'

# With idempotency key (safe for retries / repeated hook fires)
nmem t append <id> \
  -m '[{"role":"user","content":"msg"}]' \
  --idempotency-key "oc-batch-session-001"
```


### Save Claude Code / Codex / Gemini CLI session


```
nmem t save --from claude-code         # Save Claude Code session
nmem t save --from codex               # Save Codex session
nmem t save --from codex -s "Summary"  # With session summary
nmem t save --from gemini-cli           # Save Gemini CLI session
```

By default,nmemreads Claude Code from~/.claudeand Codex from~/.codex. If you keep those tools in a custom location,CLAUDE_CONFIG_DIRandCODEX_HOMEare respected automatically.
OptionDescription--fromclaude-code,codex, orgemini-cli(required)-p, --projectProject directory path (default: current dir)-m, --modecurrent(latest) orallsessions--session-idSpecific session ID (Codex only)-s, --summaryBrief session summary--truncateTruncate large tool results (>10KB)

### Delete


```
nmem t delete <id>
nmem t delete <id> -f                  # Force
nmem t delete <id> --cascade           # Also delete associated memories
```


### Move between spaces


```
nmem t move <id> --space "Research Agent" --to-space "Archive"
nmem t move <id1> <id2> --space "Research Agent" --to-space "Archive" -f
nmem t move --all-in-space "Research Agent" --to-space "Archive" --dry-run
nmem t move --all-in-space "Research Agent" --to-space "Archive" -f
```

As with memories,--spaceis the source guard and--to-spaceis the destination. Mem resolves whole-space moves on the server, so scripts do not need to page through every result first.

## Library Sources (nmem sources)

YourLibrary(PDFs, spreadsheets, Word files, presentations, Markdown, code, and URLs) is reachable from the terminal. Usenmem sourceswhen a script needs to find a document, read a passage from it, or pull structured analysis out of a spreadsheet without opening the app.
nmem sourcesaliases tonmem s.

### List, add, show, delete


```
nmem sources list                       # Sources in the current space
nmem sources add notes.pdf               # Import a file
nmem sources add https://example.com/doc # Capture a URL
nmem sources show src_abc123             # Metadata for one source
nmem sources delete src_abc123 -f        # Delete (non-interactive)
```


### Search across the Library


```
nmem sources search "meeting notes"
nmem sources search roadmap -n 10
```

Full-text match against filenames and parsed summaries. Returns source IDs you can feed into the commands below.

### Read parsed content


```
nmem sources read src_abc123                         # First 8000 characters
nmem sources read src_abc123 --offset 8000           # Next page
nmem sources read src_abc123 --limit 16000           # Larger window
```

Returns the parsed text of one document, paged by character offset. Useful for feeding a specific section into another tool.

### Search inside a single document


```
nmem sources search-chunks src_abc123 "risk factors"
nmem sources search-chunks src_abc123 budget -n 3
```

Runs a full-text search against the indexed chunks of one source. Each match comes back with the actual passage, chunk index, and score, so you and any downstream agent can see what was matched, not just how many chunks matched.

### Analyze tabular sources


```
nmem sources analyze src_abc123                                # All columns
nmem sources analyze src_abc123 --column price --column region # Specific columns
```

Runs structured analysis on CSV, TSV, XLSX, or XLS sources: column stats, distributions, and basic shape. For narrative documents, usesearch-chunksandreadinstead.
Same tools your agents usesearch,read,search-chunks, andanalyzeare the same Library tools available to AI Now, the Feed Agent, the Graph Intelligence Agent, and MCP-connected clients. Anything you can do from the terminal, agents can do on your behalf, and vice versa.

## Working Memory (nmem wm)

Working Memory is the AI-generated daily briefing: focus areas, open questions, and recent activity. Background Intelligence updates it each morning.
TheDefaultspace keeps the familiar compatibility file at~/ai-now/memory.md. If spaces are enabled,nmem wmcan read or edit the briefing for any space with--space "<space name>".

### Read


```
nmem wm                                # Today's Working Memory
nmem wm --space work                    # Working Memory for one space
nmem wm --date 2026-02-12             # Archived date
nmem wm history                        # List available archived dates
```


### Edit


```
nmem wm edit                           # Opens $EDITOR
nmem wm edit --space work              # Edit one space
nmem wm edit -m "## Focus Areas\n- Ship v0.6"   # Set directly
```


### Patch a section (non-destructive)

Replace or append to one section without touching the rest of the document:

```
# Replace a section
nmem wm patch --heading "## Focus Areas" --content "- Finish OpenClaw plugin release"

# Append to a section
nmem wm patch --heading "## Notes" --append "Reminder: deploy to staging tonight"
```

The heading is matched case-insensitively and partially."Focus"matches"## Focus Areas".

## Spaces (nmem spaces)

Use this only when you want more than the default space.

```
nmem spaces
nmem spaces create "Research Agent"
nmem spaces update "Research Agent" --instructions "Prefer research notes and hypotheses"
nmem spaces show "Research Agent"
```

If you need a stable storage key for automation,--idis still available as an advanced option. Most people should let Mem generate it.

## Graph Commands (nmem g)


### Expand graph neighborhood

Explore connected memories, entities, crystals, and source documents around a given memory:

```
nmem g expand <memory-id>
nmem g expand <memory-id> --depth 2   # Two hops out
nmem g expand <memory-id> -n 10       # Limit neighbors per hop
```


### Show EVOLVES version chain

See how a memory has been refined or superseded over time:

```
nmem g evolves <memory-id>
```


## Feed (nmem f)

The activity feed shows what was saved, learned, synthesized, or ingested, in chronological order.

```
nmem f                                  # Last 7 days (high-signal events)
nmem f --days 1                         # Today only
nmem f --days 30                        # Last 30 days
nmem f --type crystal_created           # Only crystal synthesis events
nmem f --from 2026-02-10 --to 2026-02-14   # Exact date range
nmem f --all                            # Include low-signal background events
nmem f -n 50                            # Limit events (default: 100)
```

OptionDescription--daysHow many days back (default: 7; use 1 for today)--typeFilter by event type-n, --limitMax events to fetch (default: 100)--allInclude low-signal background events--from,--toExact date range (YYYY-MM-DD)
Event types:memory_created·crystal_created·insight_generated·source_ingested·source_extracted·daily_briefing·url_captured

## Knowledge Communities (nmem c)

Browse topic clusters automatically detected in your knowledge graph:

```
nmem c                                  # List communities
nmem c -n 20
nmem c show <community-id>              # Show community details (entities, memories)
nmem c detect                            # Trigger community detection (background)
```


## Configuration & Models


### Search Index Model


```
nmem models status                      # Check current model status
nmem models download                    # Download the Search Index Model
nmem models reindex                     # Rebuild the search index
```


### LLM provider


```
nmem config provider list
nmem config provider set openai --api-key sk-xxx --model gpt-4o
nmem config provider test
```


### Processing settings


```
nmem config settings                               # Show all settings
nmem config settings set briefingHour 8            # Change morning briefing time
```


### Client connection settings

Use this when the current machine needs to connect to a remote Mem server:

```
nmem config client show
nmem config client set url https://mem.example.com
nmem config client set api-key nmem_...
nmem config client clear api-key
```

nmem config client ...updates the local client config used by this machine's CLI and plugins. It is separate fromnmem config access ..., which controls how a Mem server is exposed to other devices.

### License


```
nmem license status
nmem license activate <key> [email]   # Email is inferred from the license key when possible
nmem license renew                    # Refresh or renew this device's authorization
nmem license deactivate                 # Deactivate license on this device
```


## Remote Access


```
# LAN / private network
export NMEM_API_URL=http://192.168.1.100:14242
nmem status

# Cloudflare tunnel (from desktop app: Settings → Access Mem Anywhere)
export NMEM_API_URL=https://<your-url>
export NMEM_API_KEY=nmem_...
nmem m search "notes"

# One-off without env vars
nmem --api-url https://<your-url> status
```

VariableDescriptionDefaultNMEM_API_URLAPI server URLhttp://127.0.0.1:14242NMEM_API_KEYAPI key (Bearer auth)(unset)
Full guide:Access Mem Anywhere.

## JSON Output

Add--json(or-j) before the subcommand for machine-readable output:

```
nmem --json m search "API design" | jq '.memories[0].id'
nmem --json m add "Note" | jq -r '.id'
nmem --json f --days 1 | jq '.events[].title'
```


### Search response


```
{
  "query": "API design",
  "total": 3,
  "search_mode": "fast_bm25_vector",
  "memories": [
    {
      "id": "abc123-def456-...",
      "title": "REST API versioning decision",
      "content": "We use /v1/ prefix for all public endpoints...",
      "score": 0.91,
      "relevance_reason": "Text Match (89%) + Semantic Match (73%) | decay[imp:high]",
      "importance": 0.8,
      "labels": ["architecture", "api"],
      "event_start": "2025-09",
      "temporal_context": "past",
      "source": "cli"
    }
  ]
}
```


### Feed response


```
{
  "events": [
    {
      "id": "evt-...",
      "event_type": "memory_created",
      "severity": "info",
      "title": "Memory or event title",
      "description": "Summary text...",
      "metadata": { "source": "claude", "unit_type": "fact" },
      "related_memory_ids": ["..."],
      "created_at": "2026-02-20T02:35:28+00:00"
    }
  ]
}
```


### Error response


```
{
  "error": "api_error",
  "status_code": 404,
  "detail": "Memory not found"
}
```


## Status and Statistics


```
nmem status
# nmem v0.6.2
#   status   ok
#   api      http://127.0.0.1:14242
#   database connected
#   search   ready

nmem stats
# Database Statistics
#   memories    83
#   threads     27
#   entities    248
#   labels      177
#   communities 32
```

If search needs attention,nmem statustells you which case you are in:
- search   rebuild needed: runnmem models reindex
- search   updating metadata: Mem is filling new search metadata in the background. No rebuild is needed.

## AI Agent Integration

The--jsonflag and stable exit codes makenmemeasy to drive from AI agents.

```
# Search for context before responding
nmem --json m search "authentication flow" | jq '.memories[:3]'

# Save an insight
nmem m add "Rate limiting is per-user, not per-IP" \
  --unit-type learning --importance 0.8 -l backend

# Save a decision with when it was made
nmem m add "Chose Postgres over MySQL for task events" \
  --unit-type decision --event-start 2026-02 -l architecture

# Browse what was worked on last week
nmem --json f --days 7 | jq '.events[].title'

# Create a session thread backup
nmem t create -t "Debug session $(date +%Y%m%d)" \
  -m '[{"role":"user","content":"Investigate auth failures"},{"role":"assistant","content":"Found rate limit issue"}]'
```


## TUI

An interactive terminal UI for browsing memories, threads, and the knowledge graph:

```
nmem tui
```

If you launch the TUI withNMEM_SPACE="Research Agent", the Memories and Threads tabs stay inside that space. This is useful for agent workstations or terminal-only servers where one shell session belongs to one project or agent lane.
For moving many records between spaces, use the app or thenmem m move/nmem t movecommands. The TUI keeps browsing and editing scoped, but it does not add a separate multi-select move workflow.
In theSettingstab, you can also manageAccess Anywhere(Quick link / stable Cloudflare account mode) from terminal-only environments. This is the same remote-access feature documented inAccess Mem Anywhere.

## Troubleshooting

"command not found: nmem"
- PyPI install:pip install nmem-cli(Python 3.11+)
- Run without installing:uvx --from nmem-cli nmem --help
- macOS desktop: Settings → Preferences → Developer Tools → Install CLI
- Windows: open a new terminal after app installation
"Cannot connect to server"
1. Ensure Nowledge Mem is running
2. Try:nmem --api-url http://127.0.0.1:14242 status
3. Check for proxy or VPN blocking localhost

---

# Browse Now

> 来源: https://mem.nowledge.co/zh/docs/browse-now

browse-now是随 Nowledge Mem 桌面应用一起提供的浏览器自动化 CLI。
当智能体需要的是真实浏览器，而不是普通网页抓取时，就该用它：例如登录态网站、动态页面、表单填写、截图，或者只有完整渲染后才能看到的内容。
它和Nowledge Mem Exchange 浏览器扩展是配套工作的。扩展负责提供浏览器桥接能力，browse-now则把这条能力暴露成智能体可以直接调用的命令行工具。
如果你使用 AI Now，也可以在AI Now → 插件 → 技能中直接打开内置的Browse Now开关。打开后，AI Now 就能在合适的时候主动调用这条能力。

## 它适合做什么

遇到下面这些情况时，用browse-now：
- 网站依赖你的登录态或浏览器 Cookie
- 任务需要点击、输入、滚动或页面跳转
- 页面是动态的，普通抓取拿不到真实状态
- 智能体需要截图，或读取渲染后的页面内容
如果只是简单的公开网页查询，普通网页搜索往往更快，不必动用它。

## 使用条件

你需要同时具备：
- 已安装 Nowledge Mem 桌面应用
- 机器上可以直接调用随应用提供的browse-nowCLI
- 在 Chrome、Edge、Arc 或其他受支持的 Chromium 浏览器中安装了 Nowledge Mem Exchange 扩展
- 至少有一个已连接且扩展处于可用状态的浏览器
本机安全边界browse-now只能在本机使用。它必须运行在安装了 Nowledge Mem 应用和已连接浏览器扩展的同一台机器上。「随处访问 Mem」不会暴露浏览器桥接端点。
如果应用已经装好，browse-now通常会自动安装。你可以先检查：

```
browse-now status
browse-now --help
```


## 第一条有用路径

先拿一个你本来就会使用的网站来试：
1. 用browse-now open <url>打开页面
2. 用browse-now snapshot -i查看可交互元素
3. 按 ref 去点击或填写
4. 用browse-now get url确认是否真的跳转成功
5. 页面变化后，再运行一次snapshot -i
这就是最核心的使用顺序。

```
browse-now open https://example.com
browse-now snapshot -i
browse-now click @e5
browse-now wait 2
browse-now get url
browse-now snapshot -i
```


## 优先使用 Ref

推荐的交互顺序是：
1. snapshot -i
2. click @eN或fill @eN ...
3. get url或get title
4. 导航或 DOM 明显变化后，再次snapshot -i
如果页面的无障碍信息很弱，再考虑这些兜底方式：
- browse-now find "query"
- browse-now click -T "可见文本"
- browse-now screenshot /tmp/page.png

## 常用命令


```
browse-now open https://example.com
browse-now snapshot -i
browse-now find "search button"
browse-now click @e12
browse-now fill @e3 "Nowledge Mem" --submit
browse-now get page-text --max-chars 4000
browse-now screenshot /tmp/page.png
```


## 可靠性说明

- 来自snapshot -i的 ref 是最可靠的点击目标。
- 页面跳转后 ref 会重置，所以需要重新抓取。
- 点击成功，不代表一定能绕过登录墙、付费墙或反爬限制。
- 在判断动作是否失败之前，先确认自己到底跳到了哪个页面。

## 为智能体安装技能

让智能体学会使用browse-now，通常有两条路：
- 如果你在用 AI Now，直接打开内置的Browse Now技能开关
- 如果你在用外部智能体，安装下面的复用型npx skills技能包
如果你希望支持的智能体学会何时、怎样使用browse-now，可以安装community里的复用型技能包：

```
npx skills add nowledge-co/community/nowledge-mem-browse-now-npx-skills
```

这个技能包会教智能体：
- 遇到登录态或交互型网页任务时优先使用browse-now
- 以snapshot -i返回的 ref 作为主要操作路径
- 导航后重新抓取页面
- 用get url和get title验证结果

## 它和 Mem 的关系

browse-now不是记忆检索的替代品，它负责的是浏览器控制这一侧。
一个更完整的组合通常是：
- 先用 Nowledge Mem 的技能或集成读取上下文
- 当任务需要真实浏览器时，再调用browse-now
- 如果这次浏览器操作产出了值得长期保留的信息，再把它保存回 Mem

## PyPI 与远程使用

browse-now也发布到了 PyPI。这个发布主要是为了让你在桌面应用捆绑之外，也能安装 CLI 或 Python API。
但这并不表示浏览器自动化可以通过远程访问来暴露。即使你使用的是 PyPI 包，浏览器桥接能力仍然只保留在本机，这一点是出于安全考虑。

## 相关指南

- 浏览器扩展
- 集成
- Nowledge Mem CLI

---

# API Reference

> 来源: https://mem.nowledge.co/docs/api

The Nowledge Mem server exposes a local REST API on port14242. Every feature in the desktop app and MCP tools uses these same endpoints, so you can build your own integrations, scripts, and automations on top of the same data.
Base URL:http://127.0.0.1:14242

## Graph Visualization

Open an interactive, force-directed knowledge graph right in your browser — no desktop app required.
Checking local server…
Graph ExplorerInteractive graph explorer, embed Nowledge Mem Graph in your App.Search GraphFind relevant content and build visualization-ready graph data.Explore GraphBuild a neighborhood around one or more memory IDs with depth traversal.Sample GraphGet a representative sample of graph data for visualization.Expand NodeExpand neighbors of a specific node to get connected nodes and edges.

## Memories

Create, search, and manage your knowledge base.
Search MemoriesHybrid search with filtering, metadata, and reasoning support.List MemoriesList memories with filtering and pagination.Create MemoryCreate a new memory with automatic entity extraction.Get MemoryGet a specific memory by ID with associated labels.Update MemoryUpdate memory properties like importance, title, and content.Delete MemoryDelete a memory and optionally its relationships.Preview Memory MovePreview a bulk move between spaces before changing records.Move MemoriesMove selected memories, or all memories in one space, into another space.Delete Memory SelectionDelete selected memories, or all memories in one space, with the same space-safe selector used by the app.Export MemoryExport a memory in various formats.

### Knowledge Extraction

Extract entities and relationships from memory content into the knowledge graph.
Preview KG ExtractionPreview knowledge graph extraction for a memory before applying.Apply KG ExtractionSave extracted entities and relationships to the graph database.

### Distillation

Turn conversation threads into structured memories.
Triage ConversationLightweight check: does this conversation have save-worthy content?Preview DistillationPreview distillation results without creating memories.Distill MemoriesCreate memories from thread content after distillation.

## Threads

Import, search, and manage conversation threads from Claude Code, Codex, Cursor, and more.
List ThreadsList threads with filtering and pagination.Create ThreadCreate a new thread with messages.Search ThreadsFull thread search with message matching.Thread SummariesGet all thread titles and summaries.Get ThreadGet a thread with messages, supports pagination.Delete ThreadDelete a thread and optionally its extracted memories.Append MessagesAppend messages to an existing thread.Export ThreadExport a thread in various formats.Preview Thread MovePreview a bulk move between spaces and detect legacy conflicts.Move ThreadsMove selected threads, or all threads in one space, into another space.Delete Thread SelectionDelete selected threads, or all threads in one space, using a backend-resolved selector.Bulk DeleteDelete multiple threads at once.Parse ContentParse thread content from various formats.

### Session Import

Auto-discover and import coding sessions from AI assistants.
Discover SessionsScan for conversation files from Claude Code, Codex, Cursor, and OpenCode.Preview ConversationLoad a richer head-and-tail preview for one discovered conversation before import.Import ConversationImport an external conversation file into Nowledge Mem.Export RawExport a raw conversation file as markdown or JSON without importing.Save SessionSave coding sessions as conversation threads with deduplication.Import ThreadsImport threads from JSON messages or conversation markdown.

### Import Configuration & Watcher

Get Import ConfigGet the current import configuration.Update Import ConfigUpdate import configuration.Watcher StatusGet the status of the session watcher.Start WatcherStart auto-importing sessions.Stop WatcherStop the session watcher.Hide ProjectHide a project from the browse view.Unhide ProjectUnhide a project.Hide SessionHide a session from the browse view.Unhide SessionUnhide a session.

## Background Intelligence

The background agent that runs daily briefings, crystallization, insight detection, and more.
Agent StatusGet Background Intelligence's current status.Get Working MemoryRead the Working Memory file (today's or an archived day).Update Working MemoryWrite the Working Memory file from user edits.Working Memory HistoryList dates with archived Working Memory files.Evolution EdgesGet EVOLVES relationships between memories.

### Agent Triggers

Manually trigger agent tasks that normally run on a schedule.
Daily BriefingTrigger a daily briefing.CrystallizationTrigger a crystallization review.Insight DetectionTrigger proactive insight detection.Decay RefreshTrigger a decay score refresh.Memory CompactionTrigger a memory compaction review.Community DetectionTrigger community detection on the knowledge graph.KG ExtractionTrigger knowledge graph extraction (backfill, targeted, or scoped).Processing StatusGet knowledge processing settings and status.

### Feed Events

The event stream powering the desktop Feed view.
Get EventsGet feed events from time-partitioned JSONL files.Resolve EventResolve an action-required event with optional graph mutations.Retry EventRetry a failed background task.Delete EventSoft-delete a feed event.Stream InputStream agent processing of feed input via Wire Protocol.Persist QuestionPersist a question and agent response as a feed event.

## Spaces

Manage shared space profiles and discover the current roster across clients.
List SpacesGet the shared space roster, profile metadata, and usage.Create SpaceCreate a space profile with retrieval defaults and guidance.Get SpaceRead one space profile by name, alias, or hidden key.Update SpaceRename a space, change retrieval defaults, or update guidance.Delete SpaceRemove an empty space profile.Update Space SettingsEnable or disable spaces at the product level.

## Sources (Library)

Ingest files, URLs, and documents into the knowledge base.
List SourcesList sources with optional filtering and pagination.Search SourcesFull-text search across source names and content.Get SourceSource detail with related memories and revision chain.Source ContentRead the parsed markdown content of a source.Raw FileServe the raw source file for native preview.Delete SourceDelete a source and its search index records.Update SourceUpdate source lifecycle state (reparse, mark stale).Learn from SourceTrigger knowledge extraction from a source.Refetch URLRe-fetch a URL source's content and re-parse.Source ImageServe an extracted image from a source.

### Ingestion

Upload FileIngest a file through the full source pipeline.Ingest by PathIngest a file by local filesystem path.Batch IngestIngest a batch of files (folder import).Ingest URLFetch a URL and ingest through the source pipeline.

## Graph Analysis & Maintenance

Advanced graph operations — community detection, centrality, orphan cleanup.
Graph AnalysisComprehensive analysis including community and centrality metrics.Graph HealthHealth check for graph analysis service and algo extensions.Find OrphansFind entities with no relationships.Cleanup OrphansRemove orphaned entities from the graph.Start AugmentationStart a background job (community detection, PageRank).Augmentation StateCurrent augmentation status and parameters.Job StatusCheck progress of a specific augmentation job.List JobsList recent augmentation jobs.

## Labels & Organization

List LabelsList all labels with usage counts.Create LabelCreate a new label.Get LabelGet a specific label by ID.Update LabelUpdate an existing label.Delete LabelDelete a label and all its relationships.Memory LabelsGet labels assigned to a memory.Assign LabelAssign a label to a memory.Remove LabelRemove a label from a memory.Source LabelsGet labels assigned to a source.Assign Source LabelAssign a label to a source.Remove Source LabelRemove a label from a source.

## Favorites

Favorite MemoriesGet all favorite memories.Favorite ThreadsGet all favorite threads.Toggle Memory FavoriteToggle favorite status for a memory.Toggle Thread FavoriteToggle favorite status for a thread.

## Entities & Communities

List EntitiesList entities with optional filtering.Entity RelationshipsGet all connected entities and memories for an entity.List CommunitiesList knowledge communities with AI summaries.Community DetailsGet community details including entities and sample memories.

## Models & Search Index

Manage the embedding models and search infrastructure.
Embedding Model StatusCheck the search embedding model status.Install Embedding ModelDownload and install the search embedding model.Loaded ModelsWhich models are currently loaded in memory.Unload ModelManually unload a model from memory.Search Index StatusStatus of LanceDB and hybrid search.Reindex SearchRebuild the search index from the database.Reindex MemoriesReindex multiple memories or all needing reindex.Reindex StatusStatus of memories needing reindex.

## Storage & Data

Storage InfoOn-disk sizes for the database and search index.Optimize StorageCompact search index and flush database changes.Export DataExport all data.Import DataImport data from an export.Import StatusCheck status of a data import job.CheckpointForce a database checkpoint.

## System

Health CheckHealth check endpoint.Force CheckpointForce a database checkpoint to flush WAL to disk.User ProfileGet user profile, aliases, context, and preferred language.Thread CoverageRead-only coverage report for debugging.

## Embeddings (OpenAI-compatible)

Drop-in replacement for OpenAI's embedding API, powered by the local model.
List ModelsList available models.Create EmbeddingsGenerate embeddings using the local model.

---

# 故障排除

> 来源: https://mem.nowledge.co/zh/docs/troubleshooting


## 全部文件路径总览（桌面端 + CLI）

这里给你一个简化后的目录总览（按大目录层级）：
- 配置/状态根目录：co.nowledge.mem.desktop
- 数据根目录：NowledgeGraph
- 用户工作目录：ai-now
- 客户端工具目录（nmemCLI + OpenClaw 插件）：.nowledge-mem
macOSWindowsLinux~/Library/Application Support/co.nowledge.mem.desktop/   # 配置/状态~/Library/Application Support/NowledgeGraph/             # 数据（DB/索引/日志）~/ai-now/                                                 # 用户工作目录~/.nowledge-mem/                                          # nmem/OpenClaw 客户端配置升级后你可能仍会看到这些兼容旧路径：~/Library/Application Support/nowledge-mem/~/Library/Logs/Nowledge Graph/%APPDATA%\co.nowledge.mem.desktop\     # 配置/状态%LOCALAPPDATA%\NowledgeGraph\          # 数据（DB/索引/日志）%USERPROFILE%\ai-now\                  # 用户工作目录%USERPROFILE%\.nowledge-mem\           # nmem/OpenClaw 客户端配置~/.config/co.nowledge.mem.desktop/     # 配置/状态（XDG_CONFIG_HOME）~/.local/share/NowledgeGraph/          # 数据（XDG_DATA_HOME）~/ai-now/                              # 用户工作目录~/.nowledge-mem/                       # nmem/OpenClaw 客户端配置你也可能看到这些安装路径（取决于安装方式）：/usr/lib/nowledge-mem//usr/share/nowledge-mem/升级后你可能仍会看到这些兼容旧路径：~/.local/share/co.nowledge.mem.desktop/~/.local/share/Nowledge Graph/~/.local/share/nowledge-mem/

## 查看日志

最快方式打开设置 → 关于 → 显示日志文件，即可直接在访达或资源管理器中打开日志所在文件夹，无需使用终端。如果应用在启动时遇到错误，启动界面也会显示显示日志按钮，效果相同。
macOSWindows在 macOS 上，系统日志文件的规范路径是~/Library/Application Support/NowledgeGraph/Logs/app.log。你可以在终端中运行此命令查看：open-aConsole~/Library/Application\Support/NowledgeGraph/Logs/app.log如果你是从旧版本升级，也可能仍有旧路径：open-aConsole~/Library/Logs/Nowledge\Graph/app.log在 Windows 上，系统日志文件位于两个可能的位置，取决于安装方法：%LOCALAPPDATA%\Packages\NowledgeLabsLLC.NowledgeMem_1070t6ne485wp\logs\app.log（从 Microsoft Store 安装）%LOCALAPPDATA%\NowledgeGraph\logs\app.log（从 Nowledge Mem 网站下载的安装包安装）你可以将此粘贴到文件资源管理器的地址栏中查看：%LOCALAPPDATA%\Packages\NowledgeLabsLLC.NowledgeMem_1070t6ne485wp\logs\app.log或者：%LOCALAPPDATA%\NowledgeGraph\logs\app.log

## 搜索与索引健康检查

如果你的问题与搜索质量或搜索索引占用空间有关，先从这里开始：
- 打开设置 -> Memory Processing -> Search
- 如果搜索索引占用磁盘过大，使用Optimize
- 如果搜索结果明显不对、缺失、过时或排序异常，使用Rebuild Index
这两个操作解决的是不同问题：
- Optimize：压缩磁盘上的搜索索引占用，不需要整套重建
- Rebuild Index：从知识图谱重新生成搜索索引，适合索引状态陈旧或异常时使用
如果你是在 Linux 服务器上使用，或当前没有打开桌面端界面：
- 运行nmem status，查看搜索是已就绪、需要重建，还是只是在后台补齐元数据
- 运行nmem models status，同时查看模型安装状态和搜索索引状态
- 如果状态显示updating metadata，只需要等待，不需要手动重建

## 应用启动时间过长

症状：应用在启动期间挂起或显示超时错误。
解决方案：全局代理或 VPN 软件可能阻止应用直接访问http://127.0.0.1:14242。
配置代理/VPN 绕过配置你的代理或 VPN 工具绕过 localhost 地址。将以下内容添加到你的绕过/排除规则：127.0.0.1, localhost, ::1这允许你保持代理/VPN 启用，同时确保 Nowledge Mem 可以与其本地服务器通信。更新绕过规则后，重启 Nowledge Mem。

## Windows 启动失败：缺少 Visual C++ 运行库

症状：启动时，app.log出现：
- Import error: DLL load failed while importing _lbug
- 或Backend exited during startup readiness check: exit code: 1
原因：系统缺少数据库引擎所需的 Microsoft C++ 运行时依赖。
解决方案：
1. 下载并安装Microsoft Visual C++ Redistributable (x64)：https://aka.ms/vs/17/release/vc_redist.x64.exe
2. 安装后重启 Nowledge Mem。
3. 若仍失败，请反馈时附上app.log。

## AI Now 会话启动失败

症状：点击新任务或恢复已暂停任务时失败，AI Now 无法打开会话。
第一步：先查看 AI Now 内的启动诊断卡片。
使用 AI Now 启动诊断会话启动失败时，AI Now 会显示诊断卡片，包含：失败阶段（spawn、initialize或new_session）平台和进程退出码启动脚本最近的stderr输出可复制的诊断信息按钮点击详情展开技术字段，再点击复制诊断信息，用于反馈或提交 issue。
常见修复（尤其 Windows）：
1. 确认安装完整（嵌入式 Python 与启动脚本存在）。
2. 修改插件或模型配置后，重启 Nowledge Mem 再重试。
3. 如果你在 Windows 上装了 Conda 或其他 PowerShell 自定义，请先更新到最新版本。最近的版本已经把 AI Now 和内置nmem启动器与 PowerShell profile 钩子隔离开，避免在 Mem 自己启动前就失败。
4. 临时关闭会拦截 bundled Python / PowerShell 启动的杀毒或隔离规则。
5. 若与插件有关，在AI Now → 插件中重新连接已过期 OAuth 插件后再试。

### 可选：在 Windows 使用热键打开开发者控制台

如果 AI Now 仍然卡在会话启动，可直接使用内置热键查看日志：
- 按Ctrl+Shift+I切换 Tauri/WebView 控制台。
- 打开Console标签页。
- 使用关键词过滤日志，例如[AI Now]、[ACP]、[kimi-cli stderr]。
该方法同时适用于 Microsoft Store 安装版和官网安装包版本。
然后进入 AI Now 并点击新任务复现问题。
如果仍失败，请在反馈时附上“复制诊断信息”内容和app.log。

## 模型缓存损坏

症状：搜索、记忆提炼或知识提取功能意外停止工作。
解决方案：清除模型缓存并重新下载模型。
清除缓存导航到设置→模型，然后点击：清除缓存 (2.6GB)清除缓存后，重新下载所需的模型。

## 搜索索引占用磁盘过大

症状：搜索本身还能用，但你在设置 -> Memory Processing -> Search里看到搜索索引体积明显大得不合理。
怎么做：直接点击同一面板里的Optimize。
Optimize 会做什么Optimize会压缩磁盘上的搜索索引，并顺带刷写数据库变更。从 v0.6.8 开始，这一步会更积极地清理旧索引版本。在一些真实案例里，压缩后可能出现5 GB -> 300 MB这样的大幅下降。
适合在这些情况下使用：
1. 更新版本后，索引体积一直涨。
2. 你并没有很多内容，但搜索索引看起来异常大。
3. 搜索还能工作，但磁盘占用明显不正常。
如果执行Optimize后体积仍明显不对，再执行一次Rebuild Index。如果仍异常，反馈时请附上Memory Processing面板截图。

## 搜索结果明显不对

症状：搜索结果明显不靠谱，应该很容易搜到的记忆搜不出来，或者排序质量突然变差很多。
怎么做：打开设置 -> Memory Processing -> Search，点击Rebuild Index。
什么时候该用 Rebuild IndexRebuild Index会基于当前知识图谱完整重建搜索索引。当索引写入过程中出现中断、索引状态陈旧，或其他索引问题导致搜索质量明显下降时，这是最合适的恢复步骤。
适合在这些情况下使用：
1. 某条记忆明明存在，但用合理查询就是搜不到。
2. 升级、崩溃或大批量导入后，搜索质量突然明显变差。
3. 搜索排序结果和你确信已经存进 Mem 的内容明显对不上。
重建完成后，用同一条查询再试一次。如果结果仍明显不对，反馈时请附上查询示例，以及你预期应该出现的那条记忆。

## Windows：安装或升级后 PATH 被覆盖

症状：安装或升级 Nowledge Mem 之后，其他命令行工具突然无法使用。运行pnpm、git、node等命令时提示"不是内部或外部命令"或"command not found"。检查用户 PATH 后发现它被缩减为仅剩C:\Users\...\Nowledge Mem\cli，或者%PNPM_HOME%等环境变量引用丢失。
原因：0.6.8 之前的版本在安装过程中可能会展开 PATH 中的环境变量引用（如%PNPM_HOME%被展开为实际路径），甚至在某些情况下将整个 PATH 替换为仅包含 Nowledge Mem CLI 目录的值。
此问题已在 0.6.8 及更新版本中修复。安装程序现在会完整保留你的 PATH 条目及其环境变量引用。
如果你受到了影响，可以按以下步骤恢复 PATH：
1. 按Win+R，输入sysdm.cpl，回车。
2. 进入高级>环境变量。
3. 在用户变量下，选中Path并点击编辑。
4. 补回缺失的条目。常见的包括：%PNPM_HOME%%USERPROFILE%\AppData\Local\Programs\Microsoft VS Code\bin%USERPROFILE%\.cargo\bin%USERPROFILE%\AppData\Roaming\npm
5. 点击确定，然后打开新的终端窗口。
提示如果你不确定 PATH 里应该包含哪些条目，可以参考另一台正常的电脑，或者查阅你使用的各工具（pnpm、Node.js、Rust 等）的安装文档——每个工具的安装程序通常会说明它添加了哪个 PATH 条目。

## 找不到 CLI

症状：在终端中运行nmem返回"command not found"。
各平台解决方案：
- macOS：从设置 → 偏好设置 → 开发者工具安装 CLI
- Windows：应用安装后打开新的终端窗口（PATH 更新需要新会话）
- Windows (WSL)：参见下方WSL 设置
- Linux：CLI 包含在 deb/rpm 包中。如果手动安装，确保/usr/local/bin在你的 PATH 中
快速检查：运行nmem status以验证 CLI 可以连接到 Nowledge Mem。

## 在 WSL 中使用 nmem

如果你在 Windows 的 WSL 环境中运行 Claude Code、Codex 等编程代理，Windows 上的nmemCLI 不会直接出现在 Linux 环境中。
从 v0.6.9 起，在设置中点击安装 CLI会自动在默认 WSL 发行版中创建一个轻量桥接脚本。如果需要手动设置，在 WSL 终端中粘贴以下命令：

```
mkdir -p ~/.local/bin && cat > ~/.local/bin/nmem << 'SHIMEOF'
#!/usr/bin/env bash
cd /mnt/c || exit 1
exec cmd.exe /c nmem.cmd "$@"
SHIMEOF
chmod +x ~/.local/bin/nmem
```

这会创建一个薄封装脚本，从 Windows 挂载目录通过 WSL 互操作调用 Windows 端的nmem。这样可以避开在 WSL 主目录下常见的 UNC 路径报错。由于命令实际作为 Windows 进程运行，它会直接连接到桌面端应用的localhost——无需额外的网络配置。
验证是否正常工作：

```
nmem status
```

如果创建脚本后nmem仍然找不到，请确认~/.local/bin在你的 PATH 中。Ubuntu 默认会自动添加；其他发行版需要在~/.bashrc或~/.zshrc中加入export PATH="$HOME/.local/bin:$PATH"。
前提条件此方法依赖 WSL 互操作功能（默认已启用）。如果你在/etc/wsl.conf中设置了interop=false或appendWindowsPath=false，请重新启用，或者改用pip install nmem-cli配合随处访问 Mem。
会话捕获此桥接脚本以 Windows 进程运行nmem，因此nmem t save --from claude-code等命令会在 Windows 主目录查找会话文件，而非 WSL 主目录。实际使用中这不影响——桌面端应用会通过内置的文件监视器自动捕获 WSL 中的会话。如果你需要从 WSL 直接通过 CLI 保存会话，请改用pip install nmem-cli。

## nmem status 提示"Not Found"

症状：使用远程服务器时，nmem status显示 "Not Found — Resource doesn't exist"，但 TUI 能正常使用。
原因：CLI 访问了错误的 URL。通常是因为这台机器的客户端连接配置不存在，或者 URL 配错了。
解决方案：
1. 重新写入客户端连接配置：nmemconfigclientseturlhttps://<你的地址>nmemconfigclientsetapi-keynmem_...
2. 用 curl 验证连接：curl -H "Authorization: Bearer $NMEM_API_KEY" "$NMEM_API_URL/health"
3. 更新到最新版nmemCLI——新版本会显示更清晰的错误提示和远程配置引导。
完整流程见：随处访问 Mem。

## 远程访问返回 429

症状：nmem status或curl返回429 Too many invalid auth attempts。
解决方案：客户端多次使用了错误的 API key。
- 在设置 → 随处访问 Mem重新复制 URL + key
- 确认NMEM_API_KEY完整且没有多余空格/引号
- 如果不确定，点击Rotate生成新 key
完整流程见：随处访问 Mem。

## 远程访问返回 401 Missing API key

症状：Tunnel URL 可访问，但nmem status或curl返回401 Missing API key。
原因：某些网络代理会移除鉴权头。
解决方案：
- 升级到最新版nmem（会自动使用代理兼容回退）
- 在设置 → 随处访问 Mem重新复制 URL + key
- 手动curl可用：curl "$NMEM_API_URL/health?nmem_api_key=$NMEM_API_KEY"

## Mem 提示图谱内存已调高

症状：资料库变大后，搜索、图谱或保存开始失败，标题栏还会出现“图谱内存已更新”之类的提示。
原因：Mem 已经把下次启动要用的图谱内存调大了，但你当前这次运行还在用旧额度，所以问题会继续出现，直到你退出再打开。
解决办法：
1. 先退出，再重新打开 Nowledge Mem，让新的图谱内存额度生效。
2. 如果这个提示反复出现，去设置 → Processing → Database Tuning，把图谱内存再调大一档。
3. 如果你是无头 / 服务器部署，在启动nmem serve之前设置NOWLEDGE_KUZU_BUFFER_POOL_SIZE=512MB（或更高）。

## Linux 服务器部署时提示无法连接 127.0.0.1:14242

症状：nmem license activate、nmem models download、nmem config ...这类命令报错，说“Cannot reachhttp://127.0.0.1:14242”。
原因：这些命令需要先连上本机的 Mem 服务。fresh install 的 Linux 服务器里，最常见的情况就是服务还没启动，或者你用了nmem serve前台启动，但没有意识到后面的命令要在第二个终端里跑。
解决办法：
1. 真正的服务器场景，先装后台服务：sudo nmem service install --service-user <linux-user>
2. 然后确认服务已经起来：nmem service status和nmem status
3. 如果你只是临时用nmem serve测一下，那就保持那个终端别关，再开第二个终端执行其他nmem命令
4. 如果你要从自己电脑上的浏览器打开 Web App，先运行nmem key --show-login，然后把它打印出来的那个端口做 SSH 转发：ssh -L <port>:127.0.0.1:<port> <server>
5. 需要重新看登录 key 时，新版本用nmem key --show-login，老版本用nmem key

## 报告问题

报告 Bug发现问题或意外行为？帮助我们改进！功能请求想要 Nowledge Mem 的新功能？让我们知道！发送反馈有什么想分享的？我们在倾听。

---

# Mem Pro 计划

> 来源: https://mem.nowledge.co/zh/docs/mem-pro


## 免费 vs Pro 计划

Nowledge Mem 分免费和Pro两个计划。Pro 提供无限记忆、远程 LLM 集成（BYOK）等高级功能。详细对比见定价页面。

## 激活你的终身 Pro 许可证

从定价页面开始结账访问定价页面并点击终身 Pro按钮进入结账：前往定价页面完成付款使用你的电子邮件地址完成付款。重要此邮箱用于接收许可证密钥，并永久关联到你的 Pro 激活。收到许可证密钥你会收到一封包含许可证密钥的邮件。检索许可证你可以随时在mem.nowledge.co/licenses使用你的电子邮件地址检索许可证密钥。在应用中打开计划打开 Nowledge Mem 并导航到设置→计划：使用许可证密钥激活粘贴你的许可证密钥，然后点击激活许可证：确认 Pro 已激活激活后显示 Pro 状态：设备管理随时在mem.nowledge.co/licenses管理你的已激活设备。激活或许可证问题？联系hello@nowledge-labs.ai。

