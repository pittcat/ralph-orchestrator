---
title: fix: 为 AutoResearch 事件顺序添加工作流状态守卫
type: fix
status: active
date: 2026-05-12
origin: external - tmux-mcp/docs/report/workflow-gap-analysis-2026-05-12.md
---

# fix: 为 AutoResearch 事件顺序添加工作流状态守卫

## 概述

本计划针对一个真实的 AutoResearch 工作流故障：实验在尚未被评分的情况下就被评估了。直接症状是 Ralph 循环卡住：`LOOP_COMPLETE` 被反复拒绝，因为 `experiment.scored` 从未被发出，循环在手动修复后才关闭。

根本修复应放在 Ralph 中，而非被优化的目标项目。Ralph 目前会验证所需事件主题是否已出现，但它在循环运行期间不强制执行每个工作流或每个实验的事件顺序。本计划添加了可选的工作流状态守卫，使诸如 AutoResearch 这样的配置可以声明所需的主题序列，并让 Ralph 在乱序事件污染工作流之前将其拒绝。

## 问题框架

观察到的 AutoResearch 流程预期运行顺序为：

`experiment.planned -> experiment.ready -> experiment.measured -> experiment.scored -> experiment.evaluated`

在故障运行中，`periodic.review` 激活了 Evaluator 路径，而一个已测量的实验尚未被评分。Evaluator 发出了 `experiment.evaluated`，Ralph 接受了这个事件，因为当前引擎主要通过订阅者路由、可选的发布作用域和最终的 `required_events` 完成检查来验证事件主题。

重要的发现是：Ralph 的多帽子模式是"Hatless Ralph"：自定义帽子定义路由和指令，但执行者仍是 Ralph。在多帽子模式下，`next_hat()` 路由到 `ralph`，`build_prompt()` 可以包含从待处理事件派生的多个活跃帽子指令。这意味着仅靠 prompt 规范无法保证顺序工作流顺序。

## 需求追踪

- **R1**：Ralph 必须能够在乱序工作流事件发布到总线之前将其拒绝。
- **R2**：AutoResearch 必须能够强制 `experiment.evaluated` 不能在匹配的 `experiment.scored` 之前发生。
- **R3**：守卫必须按每个实验实例工作，而不仅仅是按全局主题。
- **R4**：没有新守卫的现有配置必须保持当前行为。
- **R5**：被拒绝的事件必须产生可操作的反馈，以便智能体可以在下一次迭代中恢复。
- **R6**：完成检查必须检测不完整的工作流实例，而不仅仅是缺失的全局主题。
- **R7**：内置的 AutoResearch 预设应在引擎支持后选择加入守卫。

## 范围边界

- 本计划针对 Ralph 编排行为，而非 `tmux-mcp` 业务逻辑。
- 本计划不尝试使帽子成为独立进程。
- 本计划不替换现有的 `required_events`；它在完成前加强了运行时路径。
- 本计划不需要新的事件总线或外部数据库。
- 本计划不应改变现有用户的默认行为，除非他们选择加入工作流守卫。

### 延后至单独任务

- 通用 AutoResearch 生成器支持：在 Ralph 支持后更新 `generate_autoresearch.py` 和模板以发出新的守卫配置。
- 丰富的 UI 可视化用于工作流守卫诊断：稍后有用，不是引擎修复的必需项。

## 上下文与研究

### 相关代码和模式

- `crates/ralph-core/src/event_loop/mod.rs`
  - `next_hat()` 在存在自定义帽子时总是返回 `ralph`（多帽子模式）。
  - `build_prompt()` 收集待处理事件并从中派生活跃的帽子。
  - `process_parse_result()` 验证发出的 JSONL 事件，应用可选的帽子发布作用域检查，记录已见主题，并将事件发布到总线。
  - `check_completion_event()` 在全局 `required_events` 缺失时拒绝 `LOOP_COMPLETE`。
- `crates/ralph-core/src/event_loop/loop_state.rs`
  - `LoopState` 跟踪 `seen_topics`，但不跟踪每个实例的工作流进度。
- `crates/ralph-core/src/config.rs`
  - `EventLoopConfig` 已拥有 `required_events`、`cancellation_promise` 和 `enforce_hat_scope`。
  - 新的可选守卫配置属于此处。
- `crates/ralph-core/src/hat_registry.rs`
  - `can_publish()` 支持作用域 enforcement，但当多个活跃帽子同时包含 Reviewer 和 Evaluator 时，作用域 enforcement 无法防止绕过。
- `crates/ralph-proto/src/event.rs`
  - 事件有 topic、payload、source、target 和 wave 元数据。目前没有通用的关联字段，所以守卫需要一种实用的关联提取策略。
- `presets/autoresearch.yml` 和 `presets/autoresearch-zh.yml`
  - 内置 AutoResearch 已表达了期望的事件链并使用 `required_events`，但缺乏运行时顺序 enforcement。

### 制度性学习

- `docs/plans/2026-05-12-001-feat-harness-extension-plan.md` 已指出事件过滤不应影响 `required_events` 或事件持久化。新守卫应遵循相同原则：仅路由和持久化通过验证的事件，并使验证行为明确。
- `docs/solutions/integration-issues/fix-claude-stream-thinking-post-event-timeout-false-failure-2026-05-06.md` 展示了编排 bug 的首选 Ralph 模式：表征确切的故障循环行为，然后添加有针对性的引擎测试，使问题不会静默返回。

### 外部参考

- 不需要外部研究。问题在于本地 Ralph 编排语义，相关行为在本地源代码和记录的工作流报告中可见。

## 关键技术决策

- 在 `event_loop` 下添加可选的 `workflow_guards` 配置。
  - 理由：这保持了向后兼容性并使 Ralph 保持简单的默认姿态。
- 将守卫建模为命名有序主题链。
  - 理由：AutoResearch 是线性阶段工作流，相同机制可应用于其他帽子管道。
- 当关联键可用时，按实例跟踪工作流进度。
  - 理由：全局的 `experiment.scored` 不应解锁不同实验的 `experiment.evaluated`。
- 在 JSONL 处理期间、在发布到总线之前拒绝乱序事件。
  - 理由：一旦无效事件到达总线，下游帽子可以在 corrupted 状态上行动。
- 拒绝时，发布恢复事件如 `task.resume` 加上诊断 payload。
  - 理由：智能体需要明确的下一个动作，循环应能恢复而不需要手动修复。
- 保持 `required_events` 作为粗粒度完成门。
  - 理由：它对向后兼容性和简单工作流仍有用的，但对于实例级顺序不足。

## 开放问题

### 规划期间已解决

- 根本修复应该在 `tmux-mcp` 中吗？
  - 决议：否。目标服务只是暴露了工作流缺口。持久修复属于 Ralph 的编排层，然后是 AutoResearch 配置生成。
- `enforce_hat_scope` 就够了吗？
  - 决议：否。在故障形态中，多个活跃帽子可以同时包含 Reviewer 和 Evaluator；作用域 enforcement 仍可允许 Evaluator 的事件。
- 仅 prompt 修复就够了吗？
  - 决议：否。prompt 更改可以减少失败，但无法在 fresh-context、多帽子 prompt 构建下保证顺序。

### 延后至实现

- 关联提取的确切语法。
  - 延后因为实现者应检查当前事件解析器工具并选择最小的兼容表示。计划要求至少支持 JSON 对象 payload，并可选择支持传统工作流的键值文本 payload。
- 确切诊断主题名称。
  - 延后因为应与 `event_loop/mod.rs` 中现有的诊断和背压命名保持一致。

## 高级技术设计

> 这说明了预期方法，是审查的方向性指导，而非实现规范。实现智能体应将其作为上下文而非要复制的代码。

受保护的 AutoResearch 状态流：

- `experiment.planned`
- `experiment.ready`
- `experiment.measured`
- `experiment.scored`
- `experiment.evaluated`

侧通道规则：

- `periodic.review` 可以保持为有效的审查信号。
- `periodic.review` 不得推进实验链。
- `experiment.evaluated` 必须被拒绝，直到相同受保护实例已达到 `experiment.scored`。

守卫应在事件发布前运行：

1. 读取 JSONL 事件。
2. 解析有效事件。
3. 应用现有的作用域和背压检查。
4. 如果未启用工作流守卫，按当前方式发布到事件总线。
5. 如果启用了工作流守卫，检查事件是否遵循配置的链。
6. 如果有效，记录实例进度并发布到事件总线。
7. 如果无效，拒绝事件并为下一个 prompt 发布恢复诊断。

## 为什么会出现乱序问题？

问题的根源在于 **Ralph 的多帽子模式（Hatless Ralph）架构**：

1. **Prompt 构建的局限性**：`build_prompt()` 会从待处理事件中收集多个活跃帽子的指令。在 AutoResearch 场景中，当 `periodic.review` 触发时，Reviewer 和 Evaluator 两个帽子可能同时处于活跃状态。

2. **事件验证的盲区**：Ralph 现有的验证机制主要关注：
   - 事件是否被正确订阅者路由
   - 可选的发布作用域检查
   - 最终的 `required_events` 完成检查

3. **时序竞争**：在故障场景中，`periodic.review` 激活了 Evaluator，而此时一个实验虽然已经 `measured`（已测量），但尚未 `scored`（已评分）。Evaluator 发出了 `experiment.evaluated`，Ralph 接受了这个事件，因为它只检查主题是否存在，而不检查**这个特定实验实例**是否已经通过了 `scored` 阶段。

## 如何解决乱序问题？

核心思路是**在事件发布到总线之前添加顺序守卫**：

### 方案概述

```
JSONL 解析 → 现有作用域/背压检查 → 【新】工作流守卫验证 → 记录 → 发布到总线
```

### 具体机制

1. **配置声明式顺序**：在 `ralph.yml` 中声明工作流的必须顺序链：
   ```yaml
   event_loop:
     workflow_guards:
       experiment_chain:
         topics: [experiment.planned, experiment.ready, experiment.measured, experiment.scored, experiment.evaluated]
         correlation_key: experiment_id  # 按实验实例隔离
   ```

2. **按实例跟踪进度**：Ralph 在 `LoopState` 中按 `chain_name + instance_key` 记录每个实验实例当前到达的阶段。只有当实例 A 已到达 `experiment.scored`，实例 A 才能发出 `experiment.evaluated`。

3. **拒绝乱序事件**：当 `experiment.evaluated` 到达时，Ralph 检查：
   - 该实验实例是否已到达 `experiment.scored`？
   - 如果否，拒绝事件并发布 `task.resume` 恢复信号

4. **侧通道隔离**：`periodic.review` 等外部信号可以继续被接受，但它们**不推进**实验链的进度。

## 实现单元

- [ ] **单元 1：添加工作流守卫配置**

**目标：** 为有序工作流链添加可选的配置界面。

**需求：** R1, R2, R3, R4

**依赖：** 无

**文件：**
- 修改：`crates/ralph-core/src/config.rs`
- 测试：`crates/ralph-core/src/event_loop/tests.rs`

**方法：**
- 在 `EventLoopConfig` 下添加默认禁用的 `workflow_guards` 部分。
- 支持一个或多个命名链。
- 每个链应定义有序主题、强制模式和可选的关联提取。
- 配置应容忍该部分的缺失而不改变现有行为。

**要遵循的模式：**
- 现有的 `required_events`、`cancellation_promise` 和 `enforce_hat_scope` 的 `EventLoopConfig` 默认值。
- `config.rs` 中现有的 serde 默认样式。

**测试场景：**
- 正常路径：没有 `workflow_guards` 的现有 YAML 在守卫禁用时反序列化。
- 正常路径：带一个工作流链的 YAML 在期望的链名称、主题顺序和强制模式下反序列化。
- 边界情况：空链列表被接受为禁用或惰性。
- 错误路径：无效链配置被 preflight 或配置验证拒绝并带有清晰消息。

**验证：**
- 现有配置测试继续通过。
- 内置预设仍能在守卫部分缺失时加载。

- [ ] **单元 2：在循环状态中跟踪工作流实例进度**

**目标：** 扩展循环状态，使 Ralph 能记住每个受保护工作流实例在其链中的位置。

**需求：** R2, R3, R6

**依赖：** 单元 1

**文件：**
- 修改：`crates/ralph-core/src/event_loop/loop_state.rs`
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
- 测试：`crates/ralph-core/src/event_loop/tests.rs`

**方法：**
- 按链名称和实例键跟踪进度。
- 仅当链明确没有关联键时才使用全局实例。
- 记录每个实例达到的最高有效阶段。
- 为遗留 `required_events` 保留现有的 `seen_topics` 行为。

**要遵循的模式：**
- `LoopState::record_event()` 和 `LoopState::missing_required_events()` 是循环生命周期事件内存的现有位置。
- 保持工作流状态数据结构小，并在事件处理期间串行更新。

**测试场景：**
- 正常路径：一个实验按顺序完成所有配置主题。
- 正常路径：两个实验 ID 独立进度。
- 边界情况：实验 1 的 `experiment.scored` 不允许实验 2 的 `experiment.evaluated`。
- 边界情况：同一实例的重复同阶段事件根据所选策略被幂等接受或一致拒绝。

**验证：**
- 工作流状态测试证明实例隔离。
- 现有的 stale-loop 和 required-event 测试保持不变。

- [ ] **单元 3：在总线发布前拒绝乱序事件**

**目标：** 防止无效下游事件（如 `experiment.evaluated` 在 `experiment.scored` 之前）到达事件总线。

**需求：** R1, R2, R5

**依赖：** 单元 1, 单元 2

**文件：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
- 测试：`crates/ralph-core/src/event_loop/tests.rs`

**方法：**
- 在 `process_parse_result()` 中现有事件解析/作用域过滤之后、`state.record_event()` 加 `bus.publish()` 之前插入工作流守卫验证。
- 当事件乱序时，不将其记录为有效的已见主题。
- 发布恢复信号，解释缺失的前置条件和受影响的链实例。
- 保持取消语义分离：`loop.cancel` 应保持为有意的中止路径。

**要遵循的模式：**
- 现有的作用域违规和背压触发的替换事件处理。
- 现有的 `LOOP_COMPLETE` 缺失 `required_events` 的拒绝行为，发布 `task.resume`。

**测试场景：**
- 正常路径：有效的 `planned -> ready -> measured -> scored -> evaluated` 事件被接受和路由。
- 错误路径：`experiment.evaluated` 在 `experiment.scored` 之前被拒绝且不记录为已见。
- 错误路径：被拒绝的事件产生可操作的恢复事件。
- 边界情况：`periodic.review` 仍可作为外部审查信号被接受，但不推进实验链。
- 集成：待处理的 `periodic.review` 加已测量的实验在评分出现前不能导致接受的 `experiment.evaluated`。

**验证：**
- 回归测试反映 `tmux-mcp` 故障：已测量实验加早期 evaluated 事件使 Evaluator 的无效决策被拒绝，循环可恢复。

- [ ] **单元 4：加强守卫链的完成验证**

**目标：** 确保 `LOOP_COMPLETE` 在受保护工作流实例不完整或损坏时不能成功。

**需求：** R5, R6

**依赖：** 单元 2, 单元 3

**文件：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
- 修改：`crates/ralph-core/src/event_loop/loop_state.rs`
- 测试：`crates/ralph-core/src/event_loop/tests.rs`

**方法：**
- 扩展完成检查以在启用守卫时咨询工作流守卫状态。
- 如果任何已启动的受保护实例未达到允许的终端阶段，则拒绝完成。
- 为无守卫配置保留现有的 `required_events` 行为和错误消息。
- 使拒绝消息以工作流术语命名缺失的阶段。

**要遵循的模式：**
- `check_completion_event()` 当前集中了持久模式、必需事件、任务完成和 scratchpad 验证。
- 保持所有完成拒绝路径一致：重置 `completion_requested` 并注入恢复上下文。

**测试场景：**
- 正常路径：当每个已启动的受保护实例达到终端阶段时完成成功。
- 错误路径：当实例有 `experiment.measured` 但没有 `experiment.scored` 时完成被拒绝。
- 错误路径：当实例有 `experiment.scored` 但没有 `experiment.evaluated`（如果链将 evaluated 标记为终端）时完成被拒绝。
- 向后兼容：当没有配置工作流守卫时完成行为不变。

**验证：**
- 现有的 `required_events` 测试通过。
- 新的守卫链完成测试覆盖不完整实例状态。

- [ ] **单元 5：更新 AutoResearch 预设以选择加入工作流守卫**

**目标：** 使 Ralph 内置的 AutoResearch 预设使用新的引擎级顺序保护。

**需求：** R2, R7

**依赖：** 单元 1 至 单元 4

**文件：**
- 修改：`presets/autoresearch.yml`
- 修改：`presets/autoresearch-zh.yml`
- 测试：`crates/ralph-cli/src/presets.rs`

**方法：**
- 添加覆盖 AutoResearch 序列的受保护实验链。
- 在预设 payload 支持的地方配置实验身份的关联提取。
- 保持现有的 `required_events` 作为额外的粗粒度完成门。
- 不改变帽子指令，除非对诊断有用才简短解释硬守卫。

**要遵循的模式：**
- `crates/ralph-cli/src/presets.rs` 中的现有预设测试。
- `presets/autoresearch.yml` 中现有的 AutoResearch 主题名称和帽子序列。

**测试场景：**
- 正常路径：内置 AutoResearch 预设反序列化并暴露配置的守卫。
- 集成：预设验证仍然通过。
- 错误路径：在聚焦事件循环测试中，AutoResearch 预设运行不能接受 `experiment.evaluated` 在 `experiment.scored` 之前。

**验证：**
- 预设测试证明新配置存在且有效。
- 现有公共预设期望继续通过。

- [ ] **单元 6：记录工作流守卫和恢复行为**

**目标：** 解释新守卫，使工作流作者无需反向工程引擎即可使用。

**需求：** R4, R5, R7

**依赖：** 单元 1 至 单元 5

**文件：**
- 修改：`docs/guide/configuration.md`
- 修改：`docs/concepts/hats-and-events.md`
- 修改：`docs/guide/presets.md`

**方法：**
- 记录何时使用工作流守卫：具有强制阶段顺序的顺序多帽子工作流。
- 澄清 `required_events`、`enforce_hat_scope` 和工作流守卫之间的区别。
- 解释 `periodic.review` 样式的侧通道事件不应推进受保护链，除非配置为主题链的一部分。
- 包含 AutoResearch 导向的示例，不嵌入实现代码。

**要遵循的模式：**
- `docs/guide/configuration.md` 中现有的配置参考表。
- `docs/concepts/hats-and-events.md` 中现有的帽子/事件概念解释。

**测试场景：**
- 测试期望：无 — 仅文档单元。

**验证：**
- 文档清晰告诉用户如何防止 `measured -> evaluated` 而没有 `scored` 的绕过。

- [ ] **单元 7：添加原始故障的重放或场景覆盖**

**目标：** 将真实故障模式捕获为回归 fixture 或场景。

**需求：** R1, R2, R5, R6

**依赖：** 单元 3, 单元 4

**文件：**
- 创建或修改：`crates/ralph-core/tests/scenarios/autoresearch_guard.yml`
- 修改：`crates/ralph-core/tests/scenarios.rs`
- 测试：`crates/ralph-core/src/event_loop/tests.rs`

**方法：**
- 添加一个模拟 AutoResearch 链的小场景，其中 `periodic.review` 在评分前交错。
- 断言无效评估被拒绝，循环被引导回评分。
- 保持场景最小；它应验证编排语义，而不是运行真正的 AutoResearch。

**要遵循的模式：**
- `crates/ralph-core/tests/scenarios/` 下的现有场景测试。
- 链验证和作用域强制的现有事件循环单元测试。

**测试场景：**
- 集成：`periodic.review` 加 `experiment.measured` 不能绕过 `experiment.scored`。
- 集成：在 `experiment.scored` 被发出后，`experiment.evaluated` 变为有效。
- 错误路径：在终端链阶段之前的 `LOOP_COMPLETE` 被拒绝并带有有用消息。

**验证：**
- 原始报告的事件顺序 bug 由失败在前、通过在后的回归测试表示。

## 系统范围影响

- **交互图：** 守卫位于 JSONL 解析和 `EventBus::publish()` 之间。仅在配置时影响下游帽子激活。
- **错误传播：** 无效事件应成为恢复诊断，而非静默丢弃。
- **状态生命周期风险：** 每个实例的工作流状态必须在内存中维持循环生命周期。跨恢复的持久化是未来增强，除非实现发现恢复需要它。
- **API 表面 parity：** CLI、预设和文档应理解新配置。初始修复不需要 Web 仪表板的一级 UI。
- **集成覆盖：** 单元测试必须覆盖配置解析、事件拒绝、完成拒绝和 AutoResearch 预设接线。
- **不变式：** 没有 `workflow_guards` 的现有工作流应表现得与今天完全一样。`required_events` 保持为主题级且向后兼容。

## 风险与依赖

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|---------|
| Payload 不一致暴露 `experiment_id` | 中 | 高 | 先支持全局链，要求 JSON payload 关联以进行实例级 enforcement，并记录传统回退限制 |
| 守卫拒绝合法恢复事件 | 中 | 中 | 仅守卫属于已配置链的主题；侧通道事件保持在链外除非显式列出 |
| 现有用户对更严格行为感到惊讶 | 低 | 中 | 使功能可选，仅在测试通过后在 AutoResearch 预设中启用 |
| 太多编排复杂性违反 Ralph 原则 | 中 | 中 | 保持机制狭窄：有序事件守卫，无新调度器，无独立帽子运行时 |
| 完成验证变得难以推理 | 中 | 中 | 在消息和测试中保持 `required_events` 和工作流守卫错误分离 |

## 文档 / 操作说明

- 发布说明应将此定位为顺序帽子工作流的可选加固功能。
- AutoResearch 文档应推荐带实验 ID 的结构化 payload，以便实例级守卫能可靠工作。
- 通用 AutoResearch 稍后应生成守卫配置并确保相关事件中存在 `experiment_id`。

## 考虑的替代方案

- 仅 prompt 修复 Reviewer 和 Evaluator 指令。
  被拒绝为不充分，因为故障是由运行时编排允许绕过造成的。
- 仅启用 `enforce_hat_scope`。
  被拒绝为不充分，因为多个活跃帽子可使 Evaluator 的发布主题在同一 prompt 中有效。
- 移除 `periodic.review`。
  被拒绝为太窄。它避免了确切的绕过但不解决一般的乱序事件问题。
- 使帽子成为独立的智能体进程。
  被拒绝为太大且与当前 Hatless Ralph 架构相反。

## 成功指标

- 乱序 AutoResearch 事件序列在 `experiment.evaluated` 到达下游消费者之前被拒绝。
- 原始 `tmux-mcp` 故障形态被回归测试覆盖。
- 现有无守卫工作流在不更改配置的情况下通过当前测试。
- 内置 AutoResearch 预设选择加入守卫并仍能验证。

## 分阶段交付

### 阶段 1：引擎守卫

- 落地配置、循环状态、事件拒绝和完成验证。
- 为有效和无效链添加有针对性的单元测试。

### 阶段 2：AutoResearch 采用

- 在 AutoResearch 预设中启用守卫。
- 为交错的 `periodic.review` 情况添加场景覆盖。

### 阶段 3：生成器跟进

- 单独更新通用 AutoResearch 生成以发出守卫和结构化实验 ID。

## 来源与参考

- 原始报告：`tmux-mcp/docs/report/workflow-gap-analysis-2026-05-12.md`
- 相关代码：`crates/ralph-core/src/event_loop/mod.rs`
- 相关代码：`crates/ralph-core/src/event_loop/loop_state.rs`
- 相关代码：`crates/ralph-core/src/config.rs`
- 相关代码：`crates/ralph-core/src/hat_registry.rs`
- 相关代码：`crates/ralph-proto/src/event.rs`
- 相关预设：`presets/autoresearch.yml`
- 相关预设：`presets/autoresearch-zh.yml`
- 相关计划：`docs/plans/2026-05-12-001-feat-harness-extension-plan.md`
