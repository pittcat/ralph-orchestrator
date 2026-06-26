## Exploration Profile Overlay — Dimension Reviewer

> **来源**:repo profile `exploration` → `ce-executor-serial/dimension-reviewer.md`
> **适用场景**:spike / 实验 / 早期原型,**目标是跑通而不是跑完美**。
> **激活方式**:`ralph run --profile repo:exploration`,或 `ralph.yml` 的 `profiles.default` 加 `repo:exploration`。

### 设计意图

Builtin preset 默认的 review 强度按 production-grade 调校——P0/P1 finding 全列、要求 reproducer、要 file:line。在**探索阶段**这套标准会**阻断前进**,因为大量"应该改"的问题根本不需要现在修。`exploration` profile 把 review 强度调低,只保留真正**会爆炸**的检查。

### P0 finding 触发条件(只在这 4 种情况报 P0)

1. **数据丢失风险**——任何 `rm -rf` / `drop table` / 不可逆写覆盖,且没有 backup / rollback 路径
2. **安全漏洞**——认证绕过、注入(SQL / 命令 / prompt)、secrets 硬编码、未授权访问
3. **不可逆损坏**——删用户文件、删数据库、删 `.git/`、`chmod -R 000` 等
4. **运行时 panic 风险**——明显的 unwrap on None / 数组越界 / 除零,在正常调用路径就能触达

### 不报 finding 的情况(主动跳过)

- 性能问题(N+1、二次扫描、内存峰值等)——记在心里,不当 blocker
- 风格 / DRY / 命名 / 注释不足
- 测试覆盖率不足(只在"完全没有测试"时报 P2,不当 P0)
- `cargo clippy --pedantic` 的 nit 级警告
- 文件 / 函数过长(除非影响可读性到难以理解)
- 测试代码本身的"丑"(探索阶段测试写得乱很正常)

### 输出约束

- finding 列表**最多 5 条**,超过只列前 5(其余进 `.ralph/agent/scratchpad.md` 留作未来清单)
- file:line **允许模糊**:"约第 3 个文件"、"investigator hat 的 instructions 段" 都接受
- **不需要** reproducer 命令
- **不需要** 建议修复方向(探索阶段方向还没定)
- P0 finding 仍需 `file:line`(否则 shipper 无法路由)

### 与其他 profile 的协作

- 与 `repo:strict` 同时启用:顺序很重要——`--profile repo:strict --profile repo:exploration` 时,exploration 拼在 strict 之后,exploration 的"只标 P0" 会**覆盖** strict 的"所有 finding 都要标",因为后者写在前面会被前者条件覆盖(若二者冲突,以**后激活**的为准——这是 v1 的设计)
- 推荐用法:`exploration` **单独**用,不要与 strict 同启——语义会混乱

### 切换时机

- 进入阶段:spike / 写 first-pass 实现 / 学习未知代码库
- 退出阶段:功能跑通 + 进入优化期 → 切到 `repo:strict` 或不传 `--profile`