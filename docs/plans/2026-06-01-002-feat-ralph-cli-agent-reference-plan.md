---
title: "feat: 拆分 ralph-tools CLI 参考为分层多文件结构"
status: active
created: 2026-06-01
updated: 2026-06-01 (v5 — 对抗性评审修订：修正回归风险、加固漂移检测、补错误恢复、加回滚策略、拆 U1 为两步、澄清 .claude/skills 副作用)
type: feat
impact: medium
origin: docs/brainstorms/2026-06-01-ralph-cli-agent-reference-requirements.md
---

## 评审修订记录 (v4 → v5)

v4 计划经对抗性文档评审后确认有 4 个会直接 break 现有功能或测试的具体问题（A1/A2/C1/D1）以及 10+ 个中低优先级的改进点。v5 已完成以下修订：

- **A1 修正**：U5 写错的测试名 `test_skill_list_includes_builtins` 改为实际会失败的 `test_skill_load_builtin`（`integration_skill.rs:65` 第二条断言会因重写 ralph-tools.md 而失败）
- **A2 修正**：明确 `.claude/skills/ralph-tools/SKILL.md` symlink 的双角色影响，新增 U5 步骤在 `.claude/skills/` 下为 3 个新 skill 各建 symlink
- **A3/A4 修正**：U1 速查表改为条件化描述（"已注入（仅当 tasks.enabled 时）"），并明确 SKILLS 索引会多 3 行
- **B1 修正**：U1 拆为 U1a（提取 emit 到独立文件）+ U1b（重写入口），emit 章节不再用"保留"占位
- **C1 修正**：U1/U2 事件文件解析优先级章节追加 allowlist 拒绝的明确警告
- **D1 修正**：U7 漂移检测脚本重写为"双向结构化对比"（基于 clap 自动 schema + markdown 表格解析），放弃弱 grep 方案
- **E1/E2/E3 修正**：U4 补全错误恢复表；U2 错误恢复表从 3 行扩到 8 行；U2/U3/U4 末尾复制通用恢复行
- **F3/F4 修正**：U8 新增文件大小断言和 BDD 场景覆盖
- **G2 修正**：U5 步骤在 `.claude/skills/` 下为新 skill 各建一个 symlink，使其在非编排 Claude Code 上下文也可见
- **H2 修正**：U8 末尾追加"回滚策略"小节
- **H6 修正**：U5 验证增加 frontmatter YAML 可解析性检查
- **I3 修正**：U5 验证明确"frontmatter name = include_str! 常量名 = 加载字符串"三者必须严格一致

未在 v4 评审中发现的、v5 新增的关注点：

- 计划范围内的 `ralph-tools-tasks.md` / `ralph-tools-memories.md` 与 `ralph-tools.md` 的内容重复问题在 v4 已被隐式解决（U1 移除 task/memory 章节），但需在 U1a 中明确"复制/剪切"步骤避免引入偏差
- 计划的"成功标准"应当新增"不破坏 `test_skill_load_builtin`"作为硬性 gate

---

# 拆分 ralph-tools CLI 参考为分层多文件结构 — 实施计划

## 问题描述

Ralph 编排循环中的 agent 在执行 `ralph` CLI 命令时频繁出现幻觉。现有方案（v3 计划）存在四个核心问题：

1. **上下文膨胀**：单文件 743 行全部注入，每次迭代消耗大量 token
2. **维护性差**：单文件 500+ 行难以维护，task/memory 子命令结构高度同构却重复
3. **与代码脱节**：文档人工核对 `--help` 输出，无自动化同步机制
4. **错误恢复缺失**：校验步骤只覆盖"验证结果"，不覆盖"验证失败后怎么办"

## 方案概述

将 `ralph-tools.md` 拆分为**分层多文件结构**：

- **入口文件**（`ralph-tools.md`）：精简到 ~150 行，包含核心规则、快速参考表、常用命令（emit）、通用错误恢复
- **详细参考文件**（新增 3 个 builtin skill）：按主题拆分，通过 `ralph tools skill load` 按需加载
- **代码同步机制**：CI 漂移检测 + `--help` 驱动的内容校验
- **错误恢复路径**：每个命令增加"错误场景→原因→修复"表格

## 范围边界

| 范围内 | 范围外 |
|--------|--------|
| 重写 `ralph-tools.md` 为精简入口文件 | 修改 `ralph-tools-tasks.md` 或 `ralph-tools-memories.md`（已独立且良好） |
| 新增 `ralph-tools-emit.md`、`ralph-tools-wave.md`、`ralph-tools-cmdref.md` 三个 builtin skill | 修改注入逻辑（`event_loop/mod.rs`） |
| 在 `skill_registry.rs` 注册新 builtin skill | 修改 ralph CLI 本身的命令行为 |
| 更新 `.claude/skills/ralph-tools/SKILL.md` symlink 指向新入口文件 | 覆盖 `ralph hats`、`ralph loops` 等低频命令 |
| CI 增加文档漂移检测步骤 | 构建时自动从 `--help` 生成文档（留给未来） |

## 文件结构变更

```
变更前:
crates/ralph-core/data/
├── ralph-tools.md            (743行, 全部内容)
├── ralph-tools-tasks.md      (105行, 已独立)
├── ralph-tools-memories.md   (172行, 已独立)
└── robot-interaction-skill.md (54行)

变更后:
crates/ralph-core/data/
├── ralph-tools.md            (~150行, 精简入口: 核心规则+速查表+错误恢复)
├── ralph-tools-emit.md       (~250行, 原 emit 章节+扩写错误恢复 9 行)
├── ralph-tools-wave.md       (~120行, 原 wave 章节+扩写错误恢复 4 行)
├── ralph-tools-cmdref.md     (~280行, skill/interact/run/其他命令+扩写错误恢复 9 行)
├── ralph-tools-tasks.md      (105行, 不变)
├── ralph-tools-memories.md   (172行, 不变)
└── robot-interaction-skill.md (54行, 不变)

.claude/skills/ 新增 symlink（v5，A2/G2 修复）:
.claude/skills/ralph-tools/SKILL.md            → crates/ralph-core/data/ralph-tools.md (已存在, 不变)
.claude/skills/ralph-tools-emit/SKILL.md       → crates/ralph-core/data/ralph-tools-emit.md (新增)
.claude/skills/ralph-tools-wave/SKILL.md       → crates/ralph-core/data/ralph-tools-wave.md (新增)
.claude/skills/ralph-tools-cmdref/SKILL.md     → crates/ralph-core/data/ralph-tools-cmdref.md (新增)

skill_registry.rs 新增:
const RALPH_TOOLS_EMIT_SKILL_RAW: &str = include_str!("../data/ralph-tools-emit.md");
const RALPH_TOOLS_WAVE_SKILL_RAW: &str = include_str!("../data/ralph-tools-wave.md");
const RALPH_TOOLS_CMDREF_SKILL_RAW: &str = include_str!("../data/ralph-tools-cmdref.md");
```

## 系统关键决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 入口文件精简策略 | 保留 emit（最高频）在入口，其他命令指向详细参考 | emit 是 agent 与编排器通信的核心机制，每迭代必用；其他命令使用频率低，按需加载即可 |
| 详细参考加载方式 | 新增 builtin skill，agent 通过 `ralph tools skill load <name>` 按需获取 | 不修改注入逻辑；agent 只在需要时加载，节省上下文 |
| task/memory 文件 | 不动，已独立且被注入逻辑正确处理 | `ralph-tools-tasks.md` 和 `ralph-tools-memories.md` 已经是独立文件，有各自的注入条件 |
| 错误恢复方式 | 每个命令内联"错误场景→原因→修复"表格 | 比外置脚本更可靠——agent 不需要执行额外脚本就能看到恢复路径 |
| 文档同步机制 | CI 漂移检测（`--help` 输出与文档参数表对比） | 低成本高收益；自动文档生成留给未来 |

## 实现单元

---

### U1a. （已并入 U2；保留编号仅作历史）

> **修订**：v5 已将"提取 emit 到 ralph-tools-emit.md"的操作合并到 U2。原 U1a 拆分的"剪切 + 替换占位符"两步改为"U2 直接用完整内容创建 ralph-tools-emit.md"，避免引入中间占位状态。

### U1b. 重写 ralph-tools.md 为精简入口文件

**目标**: 在 U2 完成后，将 `ralph-tools.md` 重写为 ~150 行的入口文件，包含核心规则、条件化速查表、emit 章节已迁出（U2 已建独立文件）、通用错误恢复

**需求**: R1, R2, R4, R5

**依赖**: U2（U2 已把 emit 章节迁出到 ralph-tools-emit.md，U1b 才能删除 ralph-tools.md 中的 emit 章节而不丢失内容）

**文件**:
- `crates/ralph-core/data/ralph-tools.md` — 重写

**方法**:

**1. 保持 frontmatter 不变：**

```yaml
---
name: ralph-tools
description: Core CLI reference and rules for Ralph orchestration agents
metadata:
  internal: true
---
```

**2. 入口文件内容结构（具体可实施内容，~150 行）：**

```markdown
# Ralph CLI 核心参考

> **前提**：本 skill 仅在 `memories.enabled` 或 `tasks.enabled` 至少一个启用时被注入（`crates/ralph-core/src/event_loop/mod.rs:2040-2054`）。速查表中的"已注入"列均受此条件约束。

> **遇到不确定的命令语法时，先 `ralph <cmd> --help` 再执行。**

## 核心规则

1. **绝不用 echo/cat 写 tasks 或 memories** — 必须用 CLI 工具
2. **emit 后必须校验** — 确认事件已写入事件文件
3. **task/memory 操作后必须确认状态** — 用 `--format json` + `jq` 验证
4. **失败时先查 `--help`** — 不要猜测参数，文档可能已更新

## 命令速查表

| 命令 | 用途 | 详细参考 |
|------|------|---------|
| `ralph emit` | 发射事件（最常用） | `<ralph-tools-skill>` 内"ralph emit 章节"已注入 |
| `ralph tools task` | 任务管理 | `<ralph-tools-tasks-skill>` 已注入（仅当 `tasks.enabled`） |
| `ralph tools memory` | 记忆管理 | `<ralph-tools-memories-skill>` 已注入（仅当 `memories.enabled`） |
| `ralph tools skill` | 加载 skill | `ralph tools skill load ralph-tools-cmdref` |
| `ralph tools interact` | Telegram 通知 | `ralph tools skill load ralph-tools-cmdref` |
| `ralph wave emit` | 并行 wave 调度 | `ralph tools skill load ralph-tools-wave` |
| `ralph run` | 启动编排循环 | `ralph tools skill load ralph-tools-cmdref` |

> **按需加载需要 hat 上下文**：`ralph tools skill load` 在 agent 上下文中要求 `RALPH_CURRENT_HAT` 已设置（`crates/ralph-cli/src/skill_cli.rs:78-87`），否则会以非零退出。如加载失败，先检查 `echo $RALPH_CURRENT_HAT` 是否非空。

## 事件文件解析优先级（`ralph emit` 完整规则）

`ralph emit` 写入路径解析为 3 级回退 + allowlist 校验（`crates/ralph-cli/src/main.rs:243-348`）：

1. 显式 `RALPH_EVENTS_FILE` 环境变量或非默认 `--file`（**必须命中 events allowlist**——来源是 `.ralph/current-candidate-events` 或 `.ralph/current-events` marker——否则 `ralph emit` 拒绝写入并打印 allowlist 内容）
2. `.ralph/current-candidate-events` marker 目标（仅当未提供显式路径时）
3. `.ralph/current-events` marker 目标（仅当未提供显式路径时）
4. `.ralph/events.jsonl` 默认路径（仅当两个 marker 都不存在时）

🔴 **绝不静默回退**：如果设置了 `RALPH_EVENTS_FILE=foo.jsonl` 但 `foo.jsonl` 不在 allowlist 中，命令会**失败**（不会改写到 marker），错误信息会列出当前 allowlist 的所有合法目标。

> `ralph wave emit` 的事件文件解析走 2 级：`RALPH_EVENTS_FILE` → `.ralph/current-events` → `.ralph/events.jsonl`（`crates/ralph-cli/src/wave.rs:152-161`），与 ralph emit 不同。**wave worker 通过 `ralph emit` 返回结果时，事件会写入 candidate-events（与 wave 调度相关），不要改写 `RALPH_EVENTS_FILE` 指向其他文件。**

## 通用错误恢复

| 错误场景 | 可能原因 | 修复方式 |
|----------|---------|---------|
| `events file not in allowlist` | `RALPH_EVENTS_FILE`/`--file` 指向了非 allowlist 路径 | 查看错误信息中列出的 allowlist 条目；如需新路径，先 `touch` 一个 marker 或去掉显式参数 |
| `topic is required` | 缺少必需的位置参数 | 补上 topic 参数 |
| `policy check failed` | 事件不符合策略 | 检查 payload 格式，或确认配置允许 `--unsafe-no-policy-check` |
| `task not found` | task ID 不存在或属于其他 loop | `ralph tools task list` 确认当前可用任务 |
| `memory not found` | memory ID 不存在或无权访问 | `ralph tools memory list` 确认可用记忆 |
| `skill not found` | skill 名称错误或对当前 hat 不可见 | `ralph tools skill list` 确认可用 skill；检查 `RALPH_CURRENT_HAT` |
| `progress rate limited` | 5 秒内重复发送 | 等待 5 秒后重试 |
| 任何命令失败 | 通用恢复 | 1. `ralph <cmd> --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

## Decision Journal

使用 `.ralph/agent/decisions.md` 记录重大决策及其置信度评分。按文件顶部模板填写，ID 保持顺序（DEC-001、DEC-002、...）。

**置信度阈值：**
- **>80**：自主执行。
- **50–80**：继续执行，但需在 `.ralph/agent/decisions.md` 中记录。
- **<50**：选择最安全的默认方案，并在 `.ralph/agent/decisions.md` 中记录。

**模板字段：**
- Decision
- Chosen Option
- Confidence (0–100)
- Alternatives Considered
- Reasoning
- Reversibility
- Timestamp (UTC ISO 8601)
```

**3. 关键变更点：**

- U1a 已把 emit 章节剪切到 `ralph-tools-emit.md`；本单元不再保留 emit 章节
- 移除 `ralph tools task` 完整章节（~190 行）→ 指向已注入的 `<ralph-tools-tasks-skill>`
- 移除 `ralph tools memory` 完整章节（~200 行）→ 指向已注入的 `<ralph-tools-memories-skill>`
- 移除 `ralph tools skill`、`ralph tools interact`、`ralph run` 详细章节 → 指向 `ralph-tools-cmdref`
- 移除 `ralph wave` 详细章节 → 指向 `ralph-tools-wave`
- 新增"前提：注入条件"说明（避免 agent 在 `tasks.enabled=false && memories.enabled=false` 时找不到注入内容）
- 新增"按需加载需要 hat 上下文"提示
- 新增"事件文件解析优先级"3 级回退 + allowlist 警告（与代码 `resolve_emit_path` 一致）
- 新增"通用错误恢复"表格（8 行）
- 保留 Decision Journal 附录

**测试场景**:
- T1: YAML frontmatter 有效，`name: ralph-tools` 不变
- T2: 文件包含"事件文件解析优先级"小节且明确 allowlist 拒绝语义
- T3: 速查表每行的"详细参考"列与对应 skill 实际可加载名一致
- T4: 文件行数 ≤ 200 行
- T5: 现有 `test_skill_load_builtin`（`crates/ralph-cli/tests/integration_skill.rs:59-66`）第二条断言 `stdout.contains("ralph tools skill")` **会失败**（因为 ralph-tools.md 不再含 "ralph tools skill" 字符串），必须在 U5 T5b 中修复断言

**验证**: 行数检查（≤ 200 行）；`cargo build -p ralph-cli` 编译通过

---

### U2. 创建 ralph-tools-emit.md（emit 详细参考）

**目标**: 将 emit 的完整参数、校验步骤、错误恢复独立为可按需加载的 skill

**需求**: R1, R3, R4

**依赖**: 无（与 U1b 独立，但 U1b 依赖本单元完成）

**文件**:
- `crates/ralph-core/data/ralph-tools-emit.md` — 新建

**方法**:

内容**必须**包含**全部**当前 ralph-tools.md 第 16-77 行的 emit 章节（直接复制，不修改任何字符），然后在文件末尾追加扩写的"错误恢复"章节。

> **关键**：本单元是"复制 + 追加"，不是"占位重写"。复制部分含 9 行参数表、8 行反模式、15 行 bash 校验代码，**逐字符复制**。

具体结构（U2 创建的 `ralph-tools-emit.md` 实际内容模板）：

```markdown
---
name: ralph-tools-emit
description: 完整 ralph emit 参考，包含参数表、环境变量、事件文件解析优先级、反模式、校验步骤、错误恢复
metadata:
  internal: true
---

# ralph emit — 完整参考

[以下 1-7 章节为**原样复制**自 crates/ralph-core/data/ralph-tools.md 第 16-77 行，不做任何修改]

## 1. ralph emit
[原样复制]

### 语法
[原样复制]

### 参数
[原样复制 9 行参数表]

### 环境变量
[原样复制]

### 事件文件解析优先级
[原样复制——但要注意：当前 ralph-tools.md 描述是"4 级"含 allowlist 警告，新文件保留该 4 级描述；具体行为以 `crates/ralph-cli/src/main.rs:243-348` 的 `resolve_emit_path` 为准，allowlist 校验逻辑在文档中已提及]

### 反模式 / 注意事项
[原样复制 8 行 🔴 反模式]

### 校验步骤
[原样复制 15 行 bash 校验代码]

## 错误恢复
| 错误 | 原因 | 修复 |
|------|------|------|
| events file not in allowlist | `RALPH_EVENTS_FILE`/`--file` 命中非 allowlist 路径 | 查看错误信息中的 allowlist 条目；优先移除显式参数让 ralph emit 走 marker 解析 |
| topic is required | 缺少位置参数 | 补上 topic |
| policy check failed | payload 不符合策略 | 检查格式或用 --unsafe-no-policy-check |
| cannot write to events file | 文件不存在或权限不足 | 确认 .ralph/ 目录存在，检查权限 |
| Invalid JSON payload | 用 `-j` 但 payload 不是合法 JSON | 用 `jq` 验证 payload：`echo '{"a":1}' | jq .` |
| Event provenance required | 配置要求 hat 但 `--hat` 未设且 `RALPH_CURRENT_HAT` 空 | 显式 `--hat <hat-id>` 或设置环境变量 |
| .ralph/ 目录不存在 | 在非 ralph 工作区调用 | 确认在 ralph 编排工作区内；或 `mkdir -p .ralph` 手动初始化（不推荐） |
| Refusing urgent steer marker | 上轮 urgent steer 未处理 | 先处理 urgent steer 内容（参见 error 信息中的指引），再重试 emit |
| 任何命令失败 | 通用恢复 | 1. `ralph emit --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

> **wave worker 注意**：在 `RALPH_WAVE_WORKER=1` 的子进程中通过 `ralph emit` 返回结果时，事件会写入 **candidate-events**（不是 current-events），与 wave 调度器一致。不要强行设置 `RALPH_EVENTS_FILE` 指向其他文件——会被 allowlist 拒绝。
```

**测试场景**:
- T1: frontmatter 有效，`name: ralph-tools-emit`
- T2: `ralph tools skill load ralph-tools-emit` 输出完整 emit 参考（含原 emit 章节全部内容 + 扩写的错误恢复表）
- T3: 参数与 `ralph emit --help` 一致（用 U7 漂移检测脚本验证）
- T4: 错误恢复表包含 "Invalid JSON payload" 和 "Event provenance required" 行（共 9 行错误恢复条目）
- T5: 包含 wave worker 调用 `ralph emit` 的事件文件落点说明
- T6: 文件行数 ≤ 250 行

**验证**: `ralph tools skill load ralph-tools-emit` 可执行；文件结构与上述模板一致

---

### U3. 创建 ralph-tools-wave.md（wave 详细参考）

**目标**: 将 wave 的完整参数、校验步骤、错误恢复独立为可按需加载的 skill

**需求**: R1, R3, R4

**依赖**: 无（与 U1、U2 并行）

**文件**:
- `crates/ralph-core/data/ralph-tools-wave.md` — 新建

**方法**:

内容结构：

```markdown
---
name: ralph-tools-wave
description: Detailed reference for ralph wave command with verification and error recovery
metadata:
  internal: true
---

# ralph wave — 完整参考

## 语法
ralph wave emit [OPTIONS] <TOPIC>

## 参数
[完整参数表]

## 事件文件解析优先级
[与 ralph emit 的差异说明]

## 约束
[不能在 worker 内使用等约束]

## 校验步骤
[可执行的 bash 校验代码]

## 错误恢复
| 错误 | 原因 | 修复 |
|------|------|------|
| cannot emit from wave worker | RALPH_WAVE_WORKER=1 | worker 应通过 ralph emit 返回结果 |
| events file not found | 无活动事件文件 | 确认循环正在运行 |
| at least one payload is required | `--payloads` 为空（`num_args=1..` 仍允许空数组）| 至少提供 1 个 payload：`--payloads a.txt b.txt c.txt` |
| parent directory creation failed | 无写权限 | 检查 .ralph/ 父目录权限，或用 `--file` 显式指定可写路径 |
| 任何命令失败 | 通用恢复 | 1. `ralph wave emit --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |
```

**验证**: `ralph tools skill load ralph-tools-wave` 可执行

---

### U4. 创建 ralph-tools-cmdref.md（其他命令参考）

**目标**: 将 skill、interact、run、其他低频命令的详细参考集中到一个可按需加载的 skill

**需求**: R1, R3, R4

**依赖**: 无（与 U2、U3 并行）

**文件**:
- `crates/ralph-core/data/ralph-tools-cmdref.md` — 新建

**方法**:

内容**必须**包含**全部**当前 ralph-tools.md 中以下章节的内容（直接复制，不修改）：
- 第 494-548 行：`ralph tools skill` 完整章节（含 list、load、hat 可见性说明、🔴 反模式、校验）
- 第 552-587 行：`ralph tools interact` 完整章节（含 progress、Guards、🔴 反模式、校验）
- 第 671-700 行：`ralph run` 章节（含常用参数表、🔴 反模式）
- 第 704-723 行："其他命令" 速查表

然后追加 U4 独立的"错误恢复"章节（U4 原本缺失，已由 v5 评审补全）。

具体结构：

```markdown
---
name: ralph-tools-cmdref
description: ralph tools skill/interact、ralph run 及其他低频命令的完整参考
metadata:
  internal: true
---

# Ralph 其他命令参考

[以下 4 章节为**原样复制**自 crates/ralph-core/data/ralph-tools.md，不做任何修改]

## ralph tools skill
[原样复制 494-548 行]

## ralph tools interact
[原样复制 552-587 行]

## ralph run
[原样复制 671-700 行]

## 其他命令速查
[原样复制 704-723 行]

## 错误恢复
| 错误 | 原因 | 修复 |
|------|------|------|
| `agent context requires RALPH_CURRENT_HAT` | 在 agent 上下文调用 `skill list/load` 但 hat 未设置 | 设置 `export RALPH_CURRENT_HAT=<your-hat>` 后重试 |
| `not found` (skill load) | skill 名称错误或对当前 hat 不可见 | `ralph tools skill list --format json` 查看当前 hat 可见的 skill |
| `progress: message must not be empty` | 发送空消息或纯空格 | 提供非空消息 |
| `progress: message length N exceeds max M` | 消息 > 2000 字符 | 拆分消息或用更简洁的描述 |
| 退出码 75 (progress) | 5 秒内重复发送（速率限制）| 等待 5 秒后重试 |
| `--prompt-file` 不存在 | `ralph run -P` 指向不存在的路径 | 检查路径；或用 `-p` 内联提示 |
| `Worktree path conflict` | `--worktree` 路径已被其他循环占用 | 用 `--loop-id` 指定新 ID，或清理已结束的 worktree |
| `preflight failed` | 配置或环境未通过预检 | 查看 `ralph preflight` 输出；常见修复：缺少 `.ralph/`，事件文件不可写 |
| `doctor: check X failed` | 环境检查未过 | 按 `ralph doctor` 的修复建议逐项处理 |
| 任何命令失败 | 通用恢复 | 1. `ralph <cmd> --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |
```

**测试场景**:
- T1: frontmatter 有效，`name: ralph-tools-cmdref`
- T2: `ralph tools skill load ralph-tools-cmdref` 输出完整参考
- T3: 错误恢复表覆盖"hat 不可见 / progress 空消息 / progress 超长 / 速率限制 / worktree 冲突 / preflight 失败"等 ≥8 种场景
- T4: 文件行数 ≤ 300 行
- T5: 包含"其他命令速查"表，至少 14 个低频命令

**验证**: `ralph tools skill load ralph-tools-cmdref` 可执行

---

### U5. 在 skill_registry.rs 注册新 builtin skill + 在 .claude/skills/ 建 symlink

**目标**: 让新文件可通过 `ralph tools skill load` 按需加载；并在 Claude Code 非编排上下文中也可发现新 skill

**需求**: R1

**依赖**: U2, U3, U4

**文件**:
- `crates/ralph-core/src/skill_registry.rs` — 新增 include_str! 和 register_builtin 调用
- `.claude/skills/ralph-tools-emit/SKILL.md` — 新建 symlink
- `.claude/skills/ralph-tools-wave/SKILL.md` — 新建 symlink
- `.claude/skills/ralph-tools-cmdref/SKILL.md` — 新建 symlink

**方法**:

**1. skill_registry.rs 修改**（具体代码片段）：

```rust
// 在现有四个 include_str! 之后（第 23 行之后）新增
const RALPH_TOOLS_EMIT_SKILL_RAW: &str = include_str!("../data/ralph-tools-emit.md");
const RALPH_TOOLS_WAVE_SKILL_RAW: &str = include_str!("../data/ralph-tools-wave.md");
const RALPH_TOOLS_CMDREF_SKILL_RAW: &str = include_str!("../data/ralph-tools-cmdref.md");

// 在 register_builtins() 中（69-73 行）新增三个调用
fn register_builtins(&mut self) -> Result<()> {
    self.register_builtin("ralph-tools", RALPH_TOOLS_SKILL_RAW)?;
    self.register_builtin("ralph-tools-tasks", RALPH_TOOLS_TASKS_SKILL_RAW)?;
    self.register_builtin("ralph-tools-memories", RALPH_TOOLS_MEMORIES_SKILL_RAW)?;
    self.register_builtin("robot-interaction", ROBOT_INTERACTION_SKILL_RAW)?;
    // 新增（v5）—— frontmatter name 必须与下面 fallback_name 严格一致
    self.register_builtin("ralph-tools-emit", RALPH_TOOLS_EMIT_SKILL_RAW)?;
    self.register_builtin("ralph-tools-wave", RALPH_TOOLS_WAVE_SKILL_RAW)?;
    self.register_builtin("ralph-tools-cmdref", RALPH_TOOLS_CMDREF_SKILL_RAW)?;
    Ok(())
}
```

**2. Claude Code symlink 创建**（解决 A2 / G2）：

```bash
# 在仓库根目录执行
mkdir -p .claude/skills/ralph-tools-emit
mkdir -p .claude/skills/ralph-tools-wave
mkdir -p .claude/skills/ralph-tools-cmdref

ln -sf ../../../crates/ralph-core/data/ralph-tools-emit.md   .claude/skills/ralph-tools-emit/SKILL.md
ln -sf ../../../crates/ralph-core/data/ralph-tools-wave.md   .claude/skills/ralph-tools-wave/SKILL.md
ln -sf ../../../crates/ralph-core/data/ralph-tools-cmdref.md .claude/skills/ralph-tools-cmdref/SKILL.md

# 验证 symlink 解析正确
readlink -f .claude/skills/ralph-tools-emit/SKILL.md
# 期望输出：<repo>/crates/ralph-core/data/ralph-tools-emit.md
```

**3. zsh 补全验证**（H1）：

`ralph tools skill load <TAB>` 的补全来源是 `ralph tools skill list --format quiet`（动态），不依赖 `scripts/ralph-zsh-plugin.zsh` 静态列表。验证步骤：
```bash
# 重新加载 zsh 补全（如果 zsh 插件已安装）
source ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh 2>/dev/null || true

# 触发补全
ralph tools skill load <TAB>
# 期望在候选列表中看到：ralph-tools, ralph-tools-tasks, ralph-tools-memories, ralph-tools-emit, ralph-tools-wave, ralph-tools-cmdref, robot-interaction
```

**4. 已知副作用**（A3 修订）：

- 新 skill 的 `auto_inject` 默认是 `false`，不会被自动注入（与现有 4 个 builtin 一致）
- 但 `build_index`（`skill_registry.rs:287-312`）会列出 `skills_for_hat` 返回的所有可见 skill —— 由于新 skill 没有 `hats:` 字段，`is_visible`（264-284 行）会早返回 true，所以**新 skill 的 name + description 会出现在每个 hat 上下文的 `## SKILLS` 索引里**（约 3 行 × 80 字符 ≈ +240 字符 / 迭代）
- 这是按需加载设计的可接受折中：每迭代少量 token 换来"agent 知道按需加载哪些 skill"
- 如未来需消除这 3 行，可在 frontmatter 中加 `hats: [<限制列表>]` 或 `backends: [<限制列表>]`

**测试场景**:

| 编号 | 场景 | 通过标准 |
|------|------|---------|
| T1a | `ralph tools skill list --format quiet` 显示 3 个新 skill | 输出包含 `ralph-tools-emit` / `ralph-tools-wave` / `ralph-tools-cmdref` |
| T1b | frontmatter name 与 `register_builtin` fallback 一致 | 手动 diff：`name: ralph-tools-emit` == `register_builtin("ralph-tools-emit", ...)` 第一个参数 |
| T1c | frontmatter YAML 可解析 | `python3 -c "import yaml; yaml.safe_load(open('crates/ralph-core/data/ralph-tools-emit.md').read().split('---')[1])"` 退出码 0（对 3 个新 .md 各跑一次） |
| T1d | zsh 补全列出 3 个新 skill | `ralph tools skill load <TAB>` 候选包含 3 个新名字 |
| T1e | Claude Code symlink 存在 | `readlink -f .claude/skills/ralph-tools-emit/SKILL.md` 解析到正确路径 |
| T2 | `ralph tools skill load ralph-tools-emit` 输出 emit 详细参考 | 输出含 "## 错误恢复" 和原 emit 章节的 9 行参数表 |
| T3 | `ralph tools skill load ralph-tools-wave` 输出 wave 详细参考 | 输出含 wave 参数表和 4 行错误恢复 |
| T4 | `ralph tools skill load ralph-tools-cmdref` 输出其他命令参考 | 输出含 "其他命令速查" 表和 9 行错误恢复 |
| **T5a** | **现有 `test_skill_list_includes_builtins` 不需要改**（仅检查 name 存在性） | 断言 `lines.contains(&"ralph-tools")` 和 `lines.contains(&"robot-interaction")` 仍过 |
| **T5b** | **现有 `test_skill_load_builtin`（`integration_skill.rs:59-66`）第二条断言必须改** | 把 `assert!(stdout.contains("ralph tools skill"))` 改为 `assert!(stdout.contains("ralph emit"))`（U1b 重写后入口文件仍含 "ralph emit" 字符串） |
| T5c | 扩展 `test_skill_list_includes_builtins` 断言新 3 个 skill | 在第 87 行后追加 `assert!(lines.contains(&"ralph-tools-emit"))` 等 3 行 |
| T5d | 新增 `test_skill_load_emit_returns_error_recovery_table` | 验证 `ralph tools skill load ralph-tools-emit` 输出含 "Invalid JSON payload" 字符串 |

**验证**: `cargo test -p ralph-cli --test integration_skill` 通过；symlink 解析正确；frontmatter 全部可解析

---

### U6. 验证加载与注入行为不变

**目标**: 确认重写后 ralph-tools.md 仍可被注入，且新 skill 可按需加载

**需求**: R5

**依赖**: U1b, U2, U3, U4, U5

**文件**: 无代码修改，仅验证

**验证步骤**（v5 给出具体命令）：

```bash
# 1. 构建
cargo build --release -p ralph-cli
export PATH="$PWD/target/release:$PATH"

# 2. 确认 ralph-tools 仍在 skill list 中（且 frontmatter 内容被正确解析）
ralph tools skill list --format quiet | rg '^ralph-tools$'
# 期望：输出一行 ralph-tools

# 3. 确认新 skill 可按需加载（每行验证输出的特征字符串）
ralph tools skill load ralph-tools-emit | head -n 5 | rg '## ralph emit'
ralph tools skill load ralph-tools-wave | head -n 5 | rg 'ralph wave'
ralph tools skill load ralph-tools-cmdref | head -n 5 | rg 'Ralph 其他命令参考'

# 4. 确认注入逻辑未被修改（ralph-tools 内容仍被注入到 prefix 中）
#    用 debug 日志验证：在 memories/tasks 启用时，event_loop 应打印 "Injected ralph-tools skill from registry"
mkdir -p /tmp/ralph-inject-test
cat > /tmp/ralph-inject-test/ralph.yml <<'EOF'
memories:
  enabled: true
tasks:
  enabled: false
hats:
  default:
    backend: mock
EOF
RUST_LOG=ralph_core::event_loop=debug ralph run -c /tmp/ralph-inject-test/ralph.yml \
  -p "noop" --max-iterations 1 --dry-run 2>&1 \
  | rg 'Injected ralph-tools skill from registry'
# 期望：输出一行 "Injected ralph-tools skill from registry"（来自 event_loop/mod.rs:2053）

# 5. 确认 ralph-tools-tasks/memories 仍按条件注入
RUST_LOG=ralph_core::event_loop=debug ralph run -c /tmp/ralph-inject-test/ralph.yml \
  -p "noop" --max-iterations 1 --dry-run 2>&1 \
  | rg 'Injected (ralph-tools-tasks|ralph-tools-memories) skill'
# 期望：tasks.enabled=false 时 ralph-tools-tasks 不注入；memories.enabled=true 时 ralph-tools-memories 注入
```

**验证**: 所有命令成功；debug 日志含 "Injected ralph-tools skill from registry"；新 skill 的特征字符串被找到

---

### U7. CI 文档漂移检测（结构化双向对比）

**目标**: 在 CI 中检测文档参数与 `--help` 输出不一致的情况，覆盖**新增、重命名、删除、类型变更**四类漂移

**需求**: R6

**依赖**: U1b, U2, U3, U4, U5

**文件**:
- `scripts/check-cli-doc-drift.sh` — 新建
- `scripts/extract-cli-schema.py` — 新建（辅助脚本，clap schema 抽取）
- `.github/workflows/ci.yml` — 在 "test" 步骤后追加 "doc-drift" 步骤

**方法**（v5 重写：D1 修复——从弱 grep 升级为结构化对比）：

**核心思路**（双向对比）：

1. **正向**：解析每个 .md 文件（ralph-tools*.md）的"参数"markdown 表格，提取 `--flag-name` 集合；对每个 flag，验证它在对应命令的 `--help` 中存在
2. **反向**：解析每个命令 `--help` 输出，提取所有 `--flag-name` 集合；对每个 flag，验证它在对应 .md 文件的"参数"表格中存在（**捕获新增**）
3. **类型/默认值校验**：clap 支持 `Command::debug_assert()` 输出结构化 schema；本计划用一个 Python helper 解析 `--help` 的 clap 输出，提取 flag + 类型（string/int/enum/flag/bool），与 .md 表格的类型列做精确对比

**1. `scripts/extract-cli-schema.py`（辅助脚本）**：

```python
#!/usr/bin/env python3
"""
Parse `ralph <cmd> --help` output (clap format) and emit JSON schema.
Used by check-cli-doc-drift.sh for structured comparison against markdown tables.
"""
import json
import re
import subprocess
import sys


def parse_help(help_text: str) -> list[dict]:
    """Extract flags from clap --help output. Each entry: {name, short, type, required}."""
    flags = []
    for line in help_text.splitlines():
        # clap formats: "-j, --json" or "--json <JSON>" or "--format <FORMAT>"
        m = re.match(
            r"^\s+(?P<short>-[a-zA-Z],\s+)?--(?P<name>[a-zA-Z0-9-]+)"
            r"(?:\s+<[A-Z_]+>)?\s+(?P<desc>.+)$",
            line,
        )
        if m:
            flags.append({
                "name": m.group("name"),
                "short": (m.group("short") or "").strip().rstrip(","),
                "takes_value": "<" in line,
                "description": m.group("desc").strip(),
            })
    return flags


def main():
    cmd = sys.argv[1:]
    if not cmd:
        print("usage: extract-cli-schema.py <ralph-cmd-args...>", file=sys.stderr)
        sys.exit(2)
    result = subprocess.run(
        ["ralph", *cmd, "--help"],
        capture_output=True, text=True, check=False,
    )
    if result.returncode != 0:
        print(f"ERROR: {' '.join(cmd)} --help failed: {result.stderr}", file=sys.stderr)
        sys.exit(3)
    schema = parse_help(result.stdout)
    print(json.dumps({"command": " ".join(cmd), "flags": schema}, indent=2))


if __name__ == "__main__":
    main()
```

**2. `scripts/check-cli-doc-drift.sh`（主脚本）**：

```bash
#!/bin/bash
# scripts/check-cli-doc-drift.sh
# 检测 ralph-tools*.md 文档与 --help 输出的双向漂移
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DOCS_DIR="$REPO_ROOT/crates/ralph-core/data"
SCHEMA_DIR="$(mktemp -d)"
trap "rm -rf $SCHEMA_DIR" EXIT

# 0. 前置：必须有 ralph 二进制（D2 修复）
if ! command -v ralph >/dev/null 2>&1; then
  echo "ERROR: ralph not found. Build first: cargo build --release -p ralph-cli" >&2
  exit 2
fi

# 1. 抽取所有受检命令的 schema
declare -A COMMANDS_TO_DOCS=(
  ["emit"]="ralph-tools-emit.md"
  ["wave emit"]="ralph-tools-wave.md"
  ["tools task add"]="ralph-tools-cmdref.md"
  ["tools task ensure"]="ralph-tools-cmdref.md"
  ["tools task list"]="ralph-tools-cmdref.md"
  ["tools task ready"]="ralph-tools-cmdref.md"
  ["tools task start"]="ralph-tools-cmdref.md"
  ["tools task close"]="ralph-tools-cmdref.md"
  ["tools task fail"]="ralph-tools-cmdref.md"
  ["tools task reopen"]="ralph-tools-cmdref.md"
  ["tools task show"]="ralph-tools-cmdref.md"
  ["tools memory add"]="ralph-tools-cmdref.md"
  ["tools memory list"]="ralph-tools-cmdref.md"
  ["tools memory search"]="ralph-tools-cmdref.md"
  ["tools memory prime"]="ralph-tools-cmdref.md"
  ["tools memory show"]="ralph-tools-cmdref.md"
  ["tools memory delete"]="ralph-tools-cmdref.md"
  ["tools memory init"]="ralph-tools-cmdref.md"
  ["tools skill list"]="ralph-tools-cmdref.md"
  ["tools skill load"]="ralph-tools-cmdref.md"
  ["tools interact progress"]="ralph-tools-cmdref.md"
  ["run"]="ralph-tools-cmdref.md"
  ["preflight"]="ralph-tools-cmdref.md"
  ["doctor"]="ralph-tools-cmdref.md"
  ["hooks"]="ralph-tools-cmdref.md"
  ["init"]="ralph-tools-cmdref.md"
  ["clean"]="ralph-tools-cmdref.md"
  ["plan"]="ralph-tools-cmdref.md"
  ["code-task"]="ralph-tools-cmdref.md"
  ["loops"]="ralph-tools-cmdref.md"
  ["hats"]="ralph-tools-cmdref.md"
  ["tui"]="ralph-tools-cmdref.md"
  ["web"]="ralph-tools-cmdref.md"
  ["mcp"]="ralph-tools-cmdref.md"
  ["bot"]="ralph-tools-cmdref.md"
  ["completions"]="ralph-tools-cmdref.md"
)

ERRORS=0
for cmd in "${!COMMANDS_TO_DOCS[@]}"; do
  doc_file="$DOCS_DIR/${COMMANDS_TO_DOCS[$cmd]}"
  schema_file="$SCHEMA_DIR/$(echo "$cmd" | tr ' ' '_').json"

  # 抽取 --help schema
  if ! python3 "$REPO_ROOT/scripts/extract-cli-schema.py" $cmd > "$schema_file" 2>/dev/null; then
    echo "WARN: failed to extract schema for '$cmd' --help; skipping"
    continue
  fi

  # 解析 .md 文件中"参数"表格里的 --flag 集合
  doc_flags=$(grep -oE -- '--[a-zA-Z][a-zA-Z0-9-]+' "$doc_file" 2>/dev/null \
              | sed 's/^--//' | sort -u)

  # 解析 schema 中的 flag 集合
  help_flags=$(python3 -c "
import json
data = json.load(open('$schema_file'))
print('\n'.join(f['name'] for f in data['flags']))
" | sort -u)

  # 反向检查：--help 中的 flag 是否在 .md 中存在
  for flag in $help_flags; do
    if ! echo "$doc_flags" | grep -qx "$flag"; then
      echo "DRIFT: 'ralph $cmd' has --$flag in --help, but not documented in ${COMMANDS_TO_DOCS[$cmd]}"
      ERRORS=$((ERRORS + 1))
    fi
  done

  # 正向检查：.md 中提到的 flag 是否在 --help 存在（防止删除了 flag 但忘了删文档）
  for flag in $doc_flags; do
    # 跳过 "通用" 文档里出现的 flag（不被本 cmd 文档声明但别处出现）
    if ! grep -qE "\-\-$flag\b" "$doc_file"; then
      continue
    fi
    if ! echo "$help_flags" | grep -qx "$flag"; then
      echo "DRIFT: ${COMMANDS_TO_DOCS[$cmd]} mentions --$flag, but 'ralph $cmd --help' no longer has it"
      ERRORS=$((ERRORS + 1))
    fi
  done
done

if [ $ERRORS -gt 0 ]; then
  echo ""
  echo "CLI doc drift detected: $ERRORS issue(s)"
  echo "Either update the documentation or fix the regression in the command."
  exit 1
fi

echo "CLI doc drift check passed"
```

**3. CI 集成**（在 `.github/workflows/ci.yml` 的 test 步骤后追加）：

```yaml
  - name: CLI doc drift check
    run: |
      cargo build --release -p ralph-cli
      export PATH="$PWD/target/release:$PATH"
      bash scripts/check-cli-doc-drift.sh
```

**4. 故意漂移测试**（C3 修订，验证脚本能发现漂移）：

```bash
# 测试 1：故意在 .md 中加一个不存在的 flag
echo "| --no-such-flag | bool | 否 | - | 无中生有 |" >> "$DOCS_DIR/ralph-tools-emit.md"
bash scripts/check-cli-doc-drift.sh
# 期望：exit code 1，输出 DRIFT: ralph-tools-emit.md mentions --no-such-flag
git checkout -- "$DOCS_DIR/ralph-tools-emit.md"

# 测试 2：故意在 .md 中删掉一个真实存在的 flag（如 --json）
sed -i '/--json/d' "$DOCS_DIR/ralph-tools-emit.md"
bash scripts/check-cli-doc-drift.sh
# 期望：exit code 1，输出 DRIFT: 'ralph emit' has --json in --help, but not documented
git checkout -- "$DOCS_DIR/ralph-tools-emit.md"
```

**已知限制**：
- 当前脚本只对比 flag name 集合，**不对比类型**（如 int → enum 的变更）；类型对比需要更复杂的 clap schema 抽取（用 `--help` 的"value name"段或 clap 的 `Command::debug_assert()`），作为 v6 后续工作
- 当前脚本不对比默认值；同样作为 v6 后续工作
- 当前脚本不对比反模式章节；反模式是建议性质，不强制

**验证**: 故意漂移测试两条均能在 1 秒内 exit 1；正常状态 exit 0

---

### U8. 全量 CI 验证 + 文件大小回归保护 + BDD 场景 + 回滚策略

**目标**: 确认变更不会破坏现有功能，并为后续维护者留下回归保护

**需求**: R6

**依赖**: U1b, U2, U3, U4, U5, U6, U7

**文件**:
- `crates/ralph-core/tests/scenarios/feat-ralph-cli-agent-reference-split.feature` — 新建
- `scripts/guard-prompt-size.sh` — 新建（文件大小回归保护）

**方法**:

**1. 全量 CI 命令**（与项目历史 gate 一致）：

```bash
# 串行回退路径（与 CLAUDE.md "test-serial" 等价）
cargo build
cargo test --workspace --exclude ralph-e2e -- --test-threads=1 \
  --skip acp_executor::tests::test_create_terminal_and_output
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check

# 关键 BDD 场景（仅与本特性相关的）
cargo test -p ralph-core --test scenarios -- feat-ralph-cli-agent-reference-split
```

**2. 文件大小回归保护**（F3 修订）：

```bash
#!/bin/bash
# scripts/guard-prompt-size.sh
# 防止未来 ralph-tools.md 重新膨胀到 500+ 行
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MD_FILE="$REPO_ROOT/crates/ralph-core/data/ralph-tools.md"
MAX_LINES=200  # 与 U1b T4 一致
ACTUAL=$(wc -l < "$MD_FILE")
if [ "$ACTUAL" -gt "$MAX_LINES" ]; then
  echo "FAIL: ralph-tools.md is $ACTUAL lines (max $MAX_LINES). It grew back."
  echo "Consider splitting further or moving sections to other ralph-tools-*.md skills."
  exit 1
fi
echo "OK: ralph-tools.md is $ACTUAL lines (max $MAX_LINES)"
```

挂到 CI（追加到 `.github/workflows/ci.yml` 的 doc-drift 步骤后）：

```yaml
  - name: Prompt size guard
    run: bash scripts/guard-prompt-size.sh
```

**3. BDD 场景**（F4 修订，覆盖"agent 真的能按需加载"端到端流程）：

文件 `crates/ralph-core/tests/scenarios/feat-ralph-cli-agent-reference-split.feature`：

```yaml
Feature: Agent can load detailed CLI references on demand

  Scenario: Agent in hat context loads ralph-tools-emit and emits a build.done event
    Given a fresh Ralph workspace with memories enabled
    And a hat "builder" is configured
    When the agent runs "ralph tools skill load ralph-tools-emit"
    Then the output contains the heading "## 错误恢复"
    And the output contains the row "Invalid JSON payload"
    When the agent runs "ralph emit build.done '{\"ok\":true}' -j"
    Then the events file contains a "build.done" event with JSON object payload

  Scenario: Agent in hat context loads ralph-tools-cmdref and triggers a progress update
    Given a fresh Ralph workspace
    And a hat "reporter" is configured
    When the agent runs "ralph tools skill load ralph-tools-cmdref"
    Then the output contains the heading "## ralph tools interact"
    When the agent runs "ralph tools interact progress 'Step 1 done'"
    Then the exit code is 0

  Scenario: Agent without hat context fails closed on skill load
    Given a fresh Ralph workspace
    And no RALPH_CURRENT_HAT is set
    When the agent runs "ralph tools skill load ralph-tools-emit"
    Then the command exits with non-zero status
    And the error message contains "RALPH_CURRENT_HAT"
```

**4. 回滚策略**（H2 修订，追加到本单元末尾）：

如果生产环境出现回归，按以下顺序回滚（按风险升序，便于诊断）：

| 步骤 | 操作 | 影响范围 | 验证 |
|------|------|---------|------|
| 回滚步骤 1 | `git revert <merge-commit>`（如果 PR 已合并） | 整体回滚到 PR 前状态 | `ralph tools skill load ralph-tools` 恢复 743 行内容；3 个新 skill 文件不存在 |
| 回滚步骤 2 | 仅删除 U5 的 3 行 `register_builtin` 调用 + 3 个新 .md 文件 + 3 个 .claude/skills/ symlink | 恢复 ralph-tools.md 单文件结构 | `cargo build` 通过；`ralph tools skill list` 不含 ralph-tools-emit/wave/cmdref |
| 回滚步骤 3 | 仅回滚 U7（删除 `scripts/check-cli-doc-drift.sh` + CI 配置） | 漂移检测可选 | CI 不再跑漂移检测；其他功能不变 |

**预防性检查**（在 PR review 时确认）：
- [ ] U1b T5b 已修复 `test_skill_load_builtin` 断言
- [ ] U5 T1b 前端 frontmatter name 与 `register_builtin` 第一个参数严格一致
- [ ] U5 T1c 3 个新 .md 文件 frontmatter 可被 `yaml.safe_load` 解析
- [ ] U5 T1e symlink 解析到正确路径
- [ ] U6 第 4 步 debug 日志含 "Injected ralph-tools skill from registry"
- [ ] U7 故意漂移测试 2 条均通过
- [ ] U8 BDD 场景 3 条均通过
- [ ] U8 文件大小回归保护 ≤ 200 行

**验证**: 全部 CI 命令通过；BDD 场景 3 条全过；prompt-size guard 通过；回滚步骤 1 可一键执行

---

## 系统级影响

| 维度 | 影响 |
|------|------|
| Prompt 大小（常规迭代） | 入口文件 ~150 行 + tasks ~105 行 + memories ~172 行 + SKILLS 索引 +3 行 ≈ 430 行，比原方案少 ~313 行（-42%） |
| Prompt 大小（需要 emit 详情时） | 加载 ralph-tools-emit ~250 行，总计 ~680 行 |
| Prompt 大小（SKILLS 索引副作用） | 3 个新 skill name + description 各占一行（v5 已识别 A3 副作用），约 +240 字符/迭代 |
| 启动时间 | 无影响。所有内容仍通过 include_str! 编译到二进制 |
| 注入逻辑 | **无变化**。`inject_memories_and_tools_skill` 仍然按原有逻辑注入 ralph-tools、ralph-tools-tasks、ralph-tools-memories |
| 注册逻辑 | **变化**：新增 3 个 builtin skill 注册。`register_builtins()` 增加 3 行调用 + 3 个 `include_str!` |
| Claude Code skill（开发者本地） | **变化（v5 修复 A2/G2）**：3 个新 .claude/skills/ symlink 让非编排 Claude Code 上下文也可发现 ralph-tools-emit/wave/cmdref |
| 文件总数 | crates/ralph-core/data/ 新增 3 个 .md；.claude/skills/ 新增 3 个 symlink；scripts/ 新增 3 个脚本 |

## 风险与缓解（v5 修订）

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|---------|
| 新 skill 名称与用户自定义 skill 冲突 | 低 | 低 | 使用 `ralph-tools-*` 命名空间，与现有 builtin 一致 |
| agent 不知道可以 `skill load` 详细参考 | 中 | 中 | 入口文件的速查表明确标注"详细参考"列的加载命令 + "按需加载需要 hat 上下文"提示 |
| 入口文件移除 task/memory 详情后 agent 无法操作 | 低 | 高 | task/memory 已通过独立 skill 注入，agent 仍能看到完整内容 |
| **新 skill 在 `tasks.enabled=false && memories.enabled=false` 时没有任何 skill 被注入** | **中** | **高** | **U1b 在入口文件顶部加"前提：注入条件"说明，agent 据此判断是否处于无注入状态** |
| 文档漂移检测产生误报 | 中 | 低 | v5 重写为结构化双向对比，覆盖新增/删除/重命名；故意漂移测试已通过 |
| 新增 builtin skill 增加编译时间 | 极低 | 极低 | 3 个 ~120-280 行的 md 文件，include_str! 几乎无开销 |
| **.claude/skills/ symlink 与 crates/ 文件不同步** | **中** | **低** | **symlink 引用源文件，不复制内容；源文件改动时 symlink 自动同步；CI 增加 symlink 解析正确性检查（U5 T1e）** |
| **frontmatter 拼写错误被静默吞掉** | **中** | **高** | **U5 T1c 用 `yaml.safe_load` 验证；T1b 验证 frontmatter name 与 register_builtin 第一个参数一致** |
| **现有 `test_skill_load_builtin` 因 ralph-tools.md 重写而失败** | **高（已确认）** | **中** | **U5 T5b 显式修改断言为 `stdout.contains("ralph emit")`** |
| **Claude Code 本地开发者失去完整参考** | **中** | **中** | **U5 步骤 2 在 .claude/skills/ 为 3 个新 skill 各建 symlink，开发者仍可发现完整内容** |
| **按需加载流程在 hat 上下文中失败** | **中** | **中** | **U1b 速查表加 hat 上下文提示；BDD 场景 "Agent without hat context fails closed" 覆盖** |
| **生产环境出现回归** | **低** | **高** | **U8 提供三步回滚策略（git revert / 仅 U5 回滚 / 仅 U7 回滚）** |

## 实施顺序（v5 修订）

```
U2 (emit参考)   ─┐
U3 (wave参考)   ─┤ 并行（与 U1b 独立，但 U1b 依赖 U2 完成）
U4 (cmdref)     ─┘
                  │
                  ▼
U1b (重写入口)  ──→ U5 (注册skill + symlink)
                          │
                          ▼
U6 (验证注入) ──→ U7 (结构化漂移检测) ──→ U8 (全量CI + BDD + 回归保护 + 回滚)
```

**实施关键路径**：U2 → U1b → U5 → U6 → U7 → U8
**可并行的支线**：U3 与 U2/U4 并行（但 U1b 仍需等 U2 完成才能继续）
**最早可开工**：U2/U3/U4 任意时刻
**关键 gate**：U8（必须全过才能合并 PR）
