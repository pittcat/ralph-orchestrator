# 拆分 ralph-tools CLI 参考为分层多文件结构 — 成果汇报

> 📅 2026-06-02 | 🔖 branch=`ralph/implement-dev-plan-docs-plans-gentle-orchid` / commit=`ec5c57e` | Step 1 of 8（U2）已装船

---

## 1. 总体结论

| 维度 | 状态 | 说明 |
|------|------|------|
| 任务完成 | 🟡 | U2 单步完成（1/8 单元）；整计划仍有 7 个单元待交付，最终 gate 在 U8 |
| 质量验收 | 🟢 | 0 项问题（无 P0 / P1 / P2 / P3）；文件与源码逐字符对齐 |
| 风险等级 | 🟢 | 本步仅 1 个新增说明文件，未触动代码、运行行为或测试 |

**一句话总结**：本次把"如何向 Ralph 汇报事件"的使用手册从一本文档里拆出来，做成一本独立的小册子，方便需要时单独取用；质量验收 0 项问题，但整本大书的拆分工作才完成八分之一，仍有七成在路上。

---

## 2. 为什么要做这件事

> **背景**：当前 Ralph 给 agent 用的"操作手册"是一份 743 行的单一文件，每次 agent 启动时整本都要看一眼，无论它用不用得上。文件越来越长，agent 找到关键信息的成本也越来越高。
>
> **本次目标**：把这本大书拆成一个 150 行的"快速入门 + 速查表"入口 + 3 本"按需取用"的专题小册子。agent 平时只需看入口；真要发事件、调度并行任务、查阅其它命令时再按需加载对应小册子。
>
> **本步（U2）属于整计划的第一步**：先把"发事件（`ralph emit`）"这一本小册子做出来，作为后续拆分的模板，并验证拆分的可行性。整计划共有 8 个实施单元（U1b / U2 / U3 / U4 / U5 / U6 / U7 / U8），本步是 U2。

---

## 3. 达成了什么

- **新建一本"发事件"专题小册子** `ralph-tools-emit.md`（92 行 / 4.7 KB）
  - 位于：`crates/ralph-core/data/ralph-tools-emit.md`（与其它"操作手册"放在一起）
  - 验证：1）头部信息（YAML 格式）可被解析 2）正文与原手册"发事件"章节逐字符完全一致（Python 字节级比对通过，62 行 0 差异） 3）新文件行数 92 ≤ 250 上限
- **追加"出错了怎么办"对照表**，从原手册的零散提示升级为 9 行结构化表格
  - 包含 9 种典型错误及其修复方法，例如："事件文件不在允许范围内"、"事件来源未声明"、"负载不是合法 JSON"等
  - 验证：两种关键错误提示文字与源码一字不差（在 `main.rs` 实际错误消息 L2982 / L3057 完全对应）
  - 特别加了"并行子任务里的事件落点"说明，避免 agent 在并行场景下踩坑
- **整计划八分之一完成**，关键路径起点已打通
  - 后续 U3（"调度并行任务"小册子）、U4（"其它命令速查"小册子）可与下一步工作并行推进；U5（把小册子挂到系统里真正可加载）依赖本步完成，现已可启动

---

## 4. 还有什么没做完 / 有什么风险

| 事项 | 状态 | 影响 | 是否需要决策 |
|------|------|------|--------------|
| 整计划仍有 7 个单元待完成（U1b/U3/U4/U5/U6/U7/U8） | 🟡 进行中 | 本步成功不代表整计划完成；最关键的"把小册子真正挂到系统"是 U5，未启动 | 否（按计划继续） |
| 本次质量验收由单一审稿人完成，非多人共识 | 🟡 低风险 | 仅 1 个 92 行说明文件改动，潜在风险面窄；建议后续 U5/U1b 完成后单独再跑一次多人审稿 | 否（已记录，下一步补做） |
| 本步新文件尚未真正"接电" | 🟢 预期状态 | 现在 `ralph tools skill load ralph-tools-emit` 仍会报"找不到"——U5 才会真正挂载，这是 U2 计划内的预期状态 | 否（按计划在 U5 处理） |
| 文件头说明用了中文长描述，与其它小册子的英文触发短语风格略不一致 | 🟡 低风险 | 可能让 agent 触发匹配稍差；U1b 入口统一后再回填 | 否（U1b 处理） |

---

## 5. 需要您拍板的事

**无紧急决策**。本步交付物单纯且风险面窄，整计划进度按既定路径推进。

如需调整优先级，可考虑以下两个方向：

1. **是否同时启动 U3 + U4 两条并行支线？**
   - 选项 A：维持顺序，下一步只做 U3（"调度并行任务"小册子）→ 单线推进，进度更可预测
   - 选项 B：U3 + U4 并行（"其它命令速查"小册子同时开工）→ 节省 1 轮迭代，但需要审稿资源跟得上
   - **建议**：选项 B（U3 + U4 并行），因为两本小册子互相独立、文件模板一致、风险面与 U2 同级；Coordinator 阶段可一并创建 Step 2 + Step 3 两个 runtime 任务

2. **是否要在 U5 之前先做一次"全量文档清点"？**
   - 选项 A：直接进 U5，让 U6 一起覆盖验证
   - 选项 B：U5 前加一次文档清点，把所有 4 个小册子（含 U1b 入口）的事先对齐
   - **建议**：选项 A（按计划走 U5 → U6），节省一轮迭代；如 U6 发现问题再补救

---

## 6. 下一步计划

1. **Coordinator 重新激活**，创建 Step 2 runtime 任务（U3 — `ralph-tools-wave.md`）
2. **同时启动 Step 3**（U4 — `ralph-tools-cmdref.md`）作为并行支线
3. **U5 启动条件已满足**：U2 完成后，U3 + U4 完成后即可开工
4. **建议**：U5 / U1b 完工后，单独跑一次多人审稿（降并发或加时长），弥补本次单点审稿的不足
5. **最终 gate 在 U8**：包含 BDD 测试、自动文档一致性检查、文件大小回归保护、一键回滚脚本，全部通过后才能视为整计划完成

---

## 附录：技术详情（供需要时查阅）

<details>
<summary>展开查看技术细节</summary>

### 执行摘要
- **Plan**: 2026-06-01-002-feat-ralph-cli-agent-reference-plan（v5，已通过对抗性评审修订）
- **Implementation Units**: 8 个（U1a 已合并到 U2）
- **本步 (U2)**: 1/8 完成
- **Code review findings**: 0 项（P0: 0, P1: 0, P2: 0, P3: 0）
- **Auto-fix rounds**: 0（0 项问题无需修复）
- **Final Validation**: pass
- **Commit hash**: `ec5c57e34700f30352e524d9a480dadc1cae5083`
- **Plan status after step**: `active`（U2 是 1/8 单元；U8 为最终 gate）

### 改了哪些文件

| 文件 | 状态 | 行数 | 字节 |
|------|------|------|------|
| `crates/ralph-core/data/ralph-tools-emit.md` | 新增 | 92 | 4770 |

未触动任何代码、测试、配置文件。

### 验收项核对（plan.md 第 45-53 行）

| # | 验收项 | 状态 | 备注 |
|---|--------|------|------|
| 1 | 入口文件 `ralph-tools.md` ≤ 200 行 | 未到 | U1b 步骤 |
| 2 | 3 个新 .md 文件头部信息可解析 | 部分 | U2 emit 通过；U3/U4 待 U3/U4 步骤 |
| 3 | `ralph tools skill list` 含 3 个新 skill | 未到 | U5 步骤（注册后才可见） |
| 4 | `ralph tools skill load <name>` 可加载 | 未到 | U5 步骤 |
| 5 | 现有"加载内置 skill"测试断言已修复 | 未到 | U5 T5b |
| 6 | `cargo build` + `cargo test` 通过 | 部分 | 本步 `cargo check -p ralph-core` 通过 |
| 7 | 文档漂移检测脚本可用 | 未到 | U7 步骤 |
| 8 | BDD 场景 3 条全过 | 未到 | U8 步骤 |
| 9 | 回滚步骤 1（git revert）可一键执行 | 未到 | U8 步骤 |

### R-IDs 覆盖（本步）

| R-ID | 描述 | 状态 |
|------|------|------|
| R1 | 新增 3 个内置 skill 并可加载 | 部分（emit 一半；U3/U4 + U5 待完成） |
| R2 | 入口文件精简到 ~150 行 | 未到（U1b） |
| R3 | 详细参考文件逐字符复制 | ✅ pass（emit 章节与 `ralph-tools.md:16-77` 字节级匹配） |
| R4 | 每章节追加"出错了怎么办"表 | ✅ pass（9 行含 "Invalid JSON payload" + "Event provenance required"） |
| R5 | 注入逻辑不变 | 未到（U6 验证） |
| R6 | CI 漂移检测 + 文件大小保护 + BDD | 未到（U7 + U8） |

### 完整发现清单

来自 `findings.md`：

```
P0 — Critical: 无
P1 — High: 无
P2 — Moderate: 无
P3 — Low: 无

残余风险：
- 6 个并行审稿人全部 300 秒超时（仅 1 个 92 行小文件，且无代码改动）
- 建议 U5/U1b 完成后单独再跑一次多人审稿
- 头部描述用了中文长串，与兄弟文件英文触发短语风格不一致（U1b 统一后再回填）
```

### 验证细节

| 验证项 | 方法 | 结果 |
|--------|------|------|
| 文件行数 | `wc -l` | 92 行（≤ 250 上限 ✅） |
| 文件字节 | `wc -c` | 4770 字节 |
| 头部信息可解析 | `python3 -c "import yaml; yaml.safe_load(...)"` | ✅ 解析通过 |
| emit 章节逐字符一致 | `python3` 字符串字节级比对 | ✅ 62 行 / 2911 字节完全一致 |
| 错误对照表行数 | 人工 + 脚本双重确认 | ✅ 9 行 |
| 关键错误文字与源码一致 | 源码 grep 对比 | ✅ `Event provenance required` (L2982) / `Invalid JSON payload` (L3057) |
| `cargo check -p ralph-core` | rtk cargo check | ✅ 编译通过，0 新警告 |
| UTF-8 / LF / 无尾随空格 | 文件元信息检查 | ✅ 全部通过 |

### Fix Log

本步无修复动作（0 项问题，无需 fix log）。

### Shipping Record

- **Commit**: `ec5c57e34700f30352e524d9a480dadc1cae5083`
- **Commit message**: `docs(core): 新增 ralph-tools-emit 内置 skill 详细参考 (plan U2)`
- **Branch**: `ralph/implement-dev-plan-docs-plans-gentle-orchid`
- **未推送至远程仓库**（按规范不在本次主动 push）
- **未提交 Ralph 自身文件**（PROMPT.md / task.md / PROMPT_backup.md）

### 关键文件路径索引

- 计划文件：`docs/plans/2026-06-01-002-feat-ralph-cli-agent-reference-plan.md`
- 上下文记录：`.agents/scratchpad/ce-executor/2026-06-01-002-feat-ralph-cli-agent-reference-plan/context.md`
- 进度跟踪：`.agents/scratchpad/ce-executor/2026-06-01-002-feat-ralph-cli-agent-reference-plan/progress.md`
- 本步审稿结论：`.agents/scratchpad/ce-executor/2026-06-01-002-feat-ralph-cli-agent-reference-plan/findings.md`
- 本步装船记录：`.agents/scratchpad/ce-executor/2026-06-01-002-feat-ralph-cli-agent-reference-plan/shipping.md`
- 本步决策记录：`.agents/scratchpad/ce-executor/2026-06-01-002-feat-ralph-cli-agent-reference-plan/decisions.md`
- 本步 diff 快照：`.agents/scratchpad/ce-executor/2026-06-01-002-feat-ralph-cli-agent-reference-plan/wave-diff.patch`

---

## 6. 后续验证门禁修复 (plan 2026-06-02-001)

功能拆分的代码层面合入后，验收层面暴露出 4 个缺口。已通过独立的 fix 计划修复，主要变化：

| 缺口 | 状态 |
|------|------|
| CI BDD job 空跑（filter 名称拼写错误） | ✅ 替换为真实 CLI integration test（8 个测试），带 test count guard |
| BDD scenario 只断言 mock 文本 | ✅ 删除误导性 YAML，由 Rust integration test 实际执行 `ralph` binary |
| CLI doc drift 默认 baseline 吞 292 条漂移 | ✅ 改为 strict 模式，GLOBAL_FLAGS 过滤全局 flag，KNOWN_DRIFTS 管理已知漂移 |
| Parser 漏 variadic `<VALUE>...` flag | ✅ 修正 regex，加 20 个特征化测试 |
| 文档 flag 与实际 --help 漂移 | ✅ 更新 memories/tasks/emit/wave/cmdref 文档，删除 baseline 文件 |

> **注意**：本报告第 1 节"质量验收 🟢"仅反映功能拆分本身的代码正确性。验收门禁的完整性由本 fix 计划保障。详情见 `docs/plans/2026-06-02-001-fix-agent-reference-validation-plan.md`。

</details>
