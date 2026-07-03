中间产物目录:
/Users/pittcat/Dev/Rust/ralph-e2e/.ralph
preset文件:
presets/en/ce-executor-serial.yml


上面分别是中间产物，还有这一个运行的 preset。你需要回答我问题，第一,整体执行过程有没有问题？第二，这个过程当中的中间产物是否符合我们的就是 RALPH 这一个基座的这些机制是否正常的生效。第三点，就是说我的编排是否合理，是否正常运行。第四点，你必须要回答我，就是说如果说真的有问题的话，那么是这个机制的问题，还是编排的问题？这个东西你要讲清楚。

你参考的代码就是参考pittcat-dev 里面的代码
# Ralph Loop Preset 运行链路诊断 Prompt

> 角色：Ralph Loop 和任意 preset 运行链路诊断专家

---

## 输入

1. **`preset_file`**：Ralph preset 文件路径（如 `ce-executor` 等）。
2. **`run_dir`**：Ralph run 实际执行目录，包含：
   - `events/`、`tasks/`
   - `context.md`、`plan.md`、`progress.md`
   - `findings.md`、`fix-log.md`
   - `report/` 等
3. **`history_docs`**：项目历史文档目录，包含：
   - `docs/plans/` — 历史开发计划
   - `docs/brainstorms/` — 历史脑暴记录
   - `docs/reviews/` — 历史 review 记录
   - `docs/report/` — 历史报告
   - `docs/achieved/` — 历史已达成事项

---

## 执行要求（Sub Agent 并行）

启动 **4 个 sub agent 并行工作**，各自职责如下：

### Agent A — 流程还原
- 读取 `preset_file`，提取预期事件流、hats、触发条件、payload schema、step 顺序。
- 读取 `run_dir`，提取实际事件、task、progress、fix-log、findings。
- 绘制**实际 vs 预期执行链路图**，标注每一步是否按预期完成（✅ / ❌ / ⏸️）。
- 输出：《执行链路对比图》

### Agent B — 历史上下文
- 读取 `history_docs` 下所有文档。
- 提取**历史问题模式**（曾遇到什么异常、根因是什么）。
- 提取**历史修复方案**（当时怎么处理、是否闭环）。
- 输出：《历史问题知识库》（按问题类型分类，附文档路径和引用）

### Agent C — 对账分析
- 基于 Agent A 的链路图 + Agent B 的历史知识库，逐项检查：
  - 每个事件 payload 是否符合 schema
  - 每个 hat 是否按触发逻辑执行
  - review / fix / ship / report 是否按 preset 规定闭环
  - task、progress、findings、fix-log 是否与预期一致
- 列出**所有偏离及证据**（文件、事件 ID、task ID、具体值）。
- 输出：《偏离证据清单》

### Agent D — 归因与修复
- 基于 Agent C 的偏离清单，逐条判断根因：
  - **preset 设计问题**（流程、事件、hats、schema 设计缺陷）
  - **Ralph Loop 基座机制问题**（事件循环、状态推进、plan-gate、queue 逻辑）
  - **agent 执行或运行产物问题**（task 输出质量、fix-log 缺失、report 不完整）
  - **多因素叠加**
- 按 **P0 / P1 / P2** 分级。
- 给出修复建议：
  - preset 问题 → 修改流程 / 事件 / schema 建议
  - Ralph loop 问题 → 机制增强建议
  - 产物问题 → task / fix-log / checklist 改进建议
- 输出：《问题归因表》+ 《修复建议》

---

## 主 Agent 职责

汇总 4 个 sub agent 输出，按以下格式输出最终报告：

### 1. 结论摘要
- 一句话总结本次 run 的健康度。
- 关键异常数量（P0 / P1 / P2）。
- 是否涉及历史重复问题。

### 2. 执行链路对比图
- 直接引用 Agent A 输出，附在报告中。

### 3. 历史问题上下文
- 直接引用 Agent B 输出，标注与本次问题的关联度（高 / 中 / 低）。

### 4. 证据清单
- 文件路径、事件 ID、task ID、payload 字段、日志片段。
- 必须标注**具体文件路径和行号 / 事件 ID**。

### 5. 问题归因表（P0 / P1 / P2）

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|--------|----------|----------|------|----------|
| P0 | ... | preset / loop / agent / 叠加 | 文件:行号 | 是/否 |
| P1 | ... | ... | ... | ... |
| P2 | ... | ... | ... | ... |

### 6. 修复建议
- 按优先级排序，每条建议附：
  - 目标文件 / 机制
  - 具体修改内容
  - 预期效果

---

## 约束

1. **每个 sub agent 只负责自己的任务，不跨职责。**
2. **Agent C 和 D 必须引用 Agent B 的历史知识库做关联分析**，避免重复踩坑。
3. **所有证据必须标注具体文件路径和行号 / 事件 ID**，不允许模糊描述。
4. **历史文档分析不可省略**，必须输出历史问题知识库，即使本次未发现直接关联。
5. **主 Agent 只做汇总和格式整理**，不重新分析原始数据。
6. **代码审查以主仓为准，中间产物目录仅作运行时产物参考。** 分析代码逻辑时必须回到主仓库（当前工作目录）的源码；只用于查看中间运行产物，核心目的是通过产物反推主仓编排逻辑是否存在缺陷，并同步检查 RALPH 机制基座本身是否存在问题。
