# Ralph Skills 使用指南

> 怎么让 agent 拥有某个技能，三种方式及其用法。

---

## 三种方式总览

| 方式 | 怎么说 | 什么时候用 |
|------|--------|-----------|
| **instructions** | 在 hat 配置里直接写指令 | 小段指令，就这个 hat 用，不需要跨项目复用 |
| **auto_inject** | 写在 skill 文件里，每次迭代自动注入 | 中等大小的文档，某些 hat 必须每次都有 |
| **SKILLS 索引** | 只有名字和描述，agent 自己决定要不要加载 | 大文档，不想每次都占 token |

三种方式可以混用。

---

## 方式一：instructions — 直接写在 hat 里

最直接的方式：指令写在 `ralph.yml` 的 hat 配置里，不创建 skill 文件。

```yaml
# ralph.yml
hats:
  builder:
    instructions: |
      ## BUILDER MODE
      你负责实现代码。每次开工前：
      1. 读 spec，搞清楚要做什么
      2. 按计划实现
      3. 写或更新测试
      4. 确保构建通过
```

指令会出现在该 hat 的 prompt 里，其他 hat 看不见。

### 用 extra_instructions 拆分 + YAML 锚点复用

指令块多了可以用 `extra_instructions` 拆分，配合 YAML 锚点在多个 hat 之间共享：

```yaml
# ralph.yml
_debug_protocol: &debug_protocol |
  ### 遇到报错时
  1. 看完整错误日志
  2. 用 `git diff` 检查最近改动
  3. 定位根因，修复，复测

hats:
  builder:
    instructions: |
      ## BUILDER MODE
      实现功能，确保构建通过。
    extra_instructions:
      - *debug_protocol        # builder 也能用调试协议

  reviewer:
    instructions: |
      ## REVIEWER MODE
      审查代码质量和正确性。
    extra_instructions:
      - *debug_protocol        # reviewer 也能用
```

YAML 锚点 (`&xxx`) 和引用 (`*xxx`) 是 YAML 自带功能，不是 Ralph 发明的，写的时候注意缩进正确。

---

## 方式二：auto_inject — 写在 skill 文件里，每次自动注入

把指令写在 `.claude/skills/` 下的 skill 文件里，Ralph 每次迭代自动塞进 prompt。

### 文件放哪

有两种放法：

```
# 方式 A：单文件
.claude/skills/ce-debug.md

# 方式 B：目录（内容放 SKILL.md 里）
.claude/skills/ce-debug/SKILL.md
```

两种方式等价，看个人喜好。

### 文件内容长什么样

```markdown
---
name: ce-debug
description: 系统化排查 bug 的工作流
hats: [builder]
name: ce-debug
description: 系统化排查 bug 的工作流
hats: [builder]
---

# ce-debug

遇到报错或测试失败时：
1. 看完整错误日志
2. 检查最近改动
3. 定位根因
4. 修复并复测
```

**说明**：
- `name` — 技能名字，唯一标识。不写的话用文件名（不含 `.md`）。
- `description` — 一句话描述。会出现在 SKILLS 索引表格里。
- `hats` — 只让某些 hat 看到。不写的话所有 hat 都能看到。
- `backends` — 只让某些后端（claude、gemini 等）看到。不写的话都行。

### auto_inject 开关在哪设？

**注意：`auto_inject` 不能写在 SKILL.md 的文件头里，只能通过 `ralph.yml` 的 `overrides` 控制。**

```yaml
# ralph.yml
skills:
  overrides:
    ce-debug:
      auto_inject: true    # 每次迭代自动注入
```

这样做的考虑：skill 文件是跨项目共享的（可以放在 `~/.claude/skills/`），但一个项目要不要自动注入、限制哪些 hat 看，是项目自己的事，不该写在 skill 文件里。

### 完整例子

`.claude/skills/ce-debug/SKILL.md`（skill 文件本身，不写 auto_inject）：

```markdown
---
name: ce-debug
description: 系统化排查 bug 的工作流
hats: [builder, reviewer]
---

# ce-debug

遇到报错或测试失败时：
1. 看完整错误日志和堆栈
2. 用对应测试框架单独跑失败用例
3. 用 `git diff` 或 `git log` 查最近改动
4. 定位根因并修复
5. 修复后复测确认通过
```

`ralph.yml`（项目控制开关）：

```yaml
skills:
  enabled: true
  dirs:
    - .claude/skills
  overrides:
    ce-debug:
      auto_inject: true

hats:
  builder:
    instructions: |
      ## BUILDER MODE
      按计划实现，写测试，确保构建通过。
  reviewer:
    instructions: |
      ## REVIEWER MODE
      审查代码质量、正确性和测试覆盖。
```

这样 builder 和 reviewer 的 prompt 里每次都会有 ce-debug 的内容。

---

## 方式三：SKILLS 索引 — 按需加载

如果 skill 内容很大（几百行文档），不想每次迭代都占用 token，可以只让它出现在 SKILLS 表格里，让 agent 需要时自己加载。

```markdown
---
name: mono-repo-standards
description: 多项目仓库的编码和提交规范（大型文档，约 500 行）
hats: [builder, reviewer]
---

（五百行的规范内容...）
```

**效果**：
- builder 和 reviewer 的 prompt 里的 SKILLS 表格能看到这项
- 但内容不会自动注入，不占 token
- agent 需要的时候执行 `ralph tools skill load mono-repo-standards` 加载
- 不在 hats 列表里的 hat（比如 planner）连 SKILLS 表格里都看不到这项

### 什么时候用按需加载

| 场景 | 推荐方式 |
|------|---------|
| 几行到几十行的小指令 | `instructions` |
| 中等大小，某些 hat 必须每次都知道 | `auto_inject` |
| 几百行的大型参考文档，agent 自己决定要不要看 | 按需加载 |

---

## 全局 Skill（跨项目共享）

默认只扫项目里的 `.claude/skills/`。想跨项目共享 skill：

```yaml
# ralph.yml
skills:
  dirs:
    - .claude/skills          # 项目级（默认）
    - ~/.claude/skills        # 全局共享
```

在 `~/.claude/skills/` 下放一份 skill，所有项目都能读到。auto_inject 和 hats 过滤照常生效。

---

## 常见问题

**Q：skill 文件的文件名和 frontmatter 里的 name 什么关系？**

frontmatter 里的 `name` 优先。没有的话用文件名（不含 `.md`）。

**Q：user 技能和内置技能重名了怎么办？**

user 技能覆盖内置技能。你可以用同名文件替换 `ralph-tools` 等内置技能的行为。

**Q：怎么看当前有哪些技能？**

```bash
ralph tools skill list
ralph tools skill list --format json    # JSON 格式
ralph tools skill list --quiet          # 只看名字
```

**Q：怎么临时禁用一个技能？**

```yaml
skills:
  overrides:
    ce-debug:
      enabled: false
```
