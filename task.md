/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-clever-swan/.ralph,@presets/en/ce-executor-isolated.yml
你现在是 Ralph Loop 和任意 preset 运行链路诊断专家。

我会提供两个输入：
1. 一个 Ralph preset 文件（可能是 ce-executor 或其他）。
2. Ralph run 的实际执行目录，包含事件、task 状态、context.md、plan.md、progress.md、findings.md、fix-log.md、report 等。

请按下面步骤分析：

1. **流程还原**
   - 从 preset 文件读取预期事件流、hats、每个事件的触发条件、payload schema、step 顺序。
   - 从运行目录读取实际事件、task、progress 状态、fix-log、findings。
   - 绘制实际 vs 预期执行链路图，标注每一步是否按预期完成。

2. **对账分析**
   - 检查每个事件 payload 是否符合 schema。
   - 检查每个 hat 是否按触发逻辑执行。
   - 检查 review/fix/ship/report 是否按 preset 规定闭环。
   - 检查 task、progress、findings、fix-log 是否与预期一致。
   - 列出任何偏离或异常及对应证据。

3. **问题归因**
   - 对于每个异常或偏离，判断：
     - preset 设计问题（流程、事件、hats、schema）
     - Ralph Loop 基座机制问题（事件循环、状态推进、plan-gate、queue 逻辑）
     - agent 执行或运行产物问题
     - 多因素叠加

4. **修复建议**
   - 针对 preset 问题 → 修改流程/事件/schema 建议
   - 针对 Ralph loop 问题 → 提供机制增强建议
   - 针对产物问题 → 给出 task/fix-log/checklist 改进建议

输出格式：
- 结论摘要
- 执行链路对比图
- 证据清单（文件、事件、task、payload、logs）
- 问题归因表（P0/P1/P2）
- 修复建议


