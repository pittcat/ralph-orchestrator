---
date: 2026-06-11
topic: multi-hat-isolated-mode
---

# Multi-Hat Isolated Mode Requirements

## Summary

凡 `hats:` 下配置项总数达到 4 个的 preset，必须显式使用 isolated execution mode。该规则通过静态 lint 与运行前 preflight 双重硬门禁执行；所有现行内置超阈值 preset 同步迁移，并删除旧 `ce-executor`，仅保留 `ce-executor-isolated`。

---

## Problem Frame

复杂 coordinator preset 会把多个 hat 的指令和可见事件组合到同一个 backend 执行上下文。实际运行中，hat 数超过 3 个后已频繁出现角色边界模糊、业务状态由错误 hat 发布、工作流阶段被错误推进等问题。

isolated mode 将每个 hat 放入独立 backend execution，prompt 只包含目标 hat 的指令和事件，并按目标 hat 的 `publishes` 约束普通业务事件。它能从执行上下文层面消除复杂 coordinator topology 的主要冒发来源。

仅迁移内置 preset 不足以形成长期约束：自定义复杂 preset 仍可能继续使用 coordinator mode。因此需要把“4 个及以上 hat 必须 isolated”定义为统一、不可豁免的 preset 合法性规则。

同时，现有 isolated mode 仍存在终态事件权限绕过、pending hat 调度饥饿和复杂拓扑回归覆盖不足。全面迁移必须以这些机制缺口完成加固为前置条件。

---

## Key Decisions

- **固定阈值，而非拓扑推断。** 直接计算 `hats:` 下配置项总数；总数达到 4 个即触发规则，不排除 aggregate、observer 或其他特殊 hat。
- **显式配置，而非自动切换。** 超阈值 preset 必须声明 isolated mode。Ralph 不在加载或运行期间静默改变用户配置。
- **不可豁免。** 不提供生产、测试或危险开关绕过；不满足规则的配置均视为无效。
- **双重硬门禁。** 静态 lint 负责作者反馈，preflight 负责阻止绕过 lint 的实际运行。
- **删除旧 preset。** 删除 `ce-executor`，仅保留 `ce-executor-isolated`；旧名称不提供别名、静默映射或兼容执行入口。
- **先加固，再全面迁移。** isolated mode 的终态权限和调度公平性必须在内置 preset 全量切换前达到验收条件。

---

## Requirements

### Multi-Hat Policy

- R1. 当 `hats:` 下配置项总数小于等于 3 时，preset 可以选择 coordinator 或 isolated mode。
- R2. 当 `hats:` 下配置项总数达到 4 个时，preset 必须显式声明 isolated execution mode。
- R3. 未声明 execution mode 按 coordinator 处理，因此超阈值时必须失败，不能依赖默认值通过门禁。
- R4. hat 计数必须基于配置项总数，不根据 trigger 可达性、并发度、aggregate、observer、backend 或运行阶段调整。
- R5. 该规则不允许配置豁免、环境变量豁免或隐藏兼容开关。

### Validation Gates

- R6. Preset 静态 lint 必须对违反 R2 的配置产生 error 级 finding，而不是 warning。
- R7. Preflight 必须独立执行同一规则，并在发现违规时拒绝启动 loop。
- R8. Lint 与 preflight 必须使用同一规则定义，保证阈值、计数口径和错误分类一致。
- R9. 错误结果必须至少包含实际 hat 数、允许的 coordinator 上限及改用 isolated mode 的修复方向。
- R10. 内置 preset 构建或测试必须验证所有超阈值 preset 均通过该规则，防止后续新增 preset 绕过门禁。

### Builtin Preset Migration

- R11. 下列当前超阈值内置 preset 必须迁移为 isolated mode：`autoresearch`、`ce-executor-wave`、`code-assist`、`debug`、`merge-loop`、`pdd-to-code-assist`。
- R12. `ce-executor-isolated` 保持 isolated mode，并成为原 `ce-executor` 使用场景的唯一现行入口。
- R13. 删除内置 `ce-executor` preset 及其实际配置、manifest、公开索引、注册表和 shell completion 入口。
- R14. 对 `builtin:ce-executor` 的现行调用必须明确失败，不得映射到 `builtin:ce-executor-isolated`。
- R15. 所有现行指南、示例、模板、测试输入和命令说明必须改用 `ce-executor-isolated`；历史归档可保留旧名称作为事实记录。
- R16. 内置 preset 迁移不得改变原 topology、业务事件协议或职责划分，除非 isolated mode 的单-hat 执行语义要求修正原本依赖 coordinator 共享上下文的行为。

### Isolated Terminal Authority

- R17. Completion promise、最终 review verdict、report completion 等终态事件不得因被归类为 system event 而绕过 hat 发布权限校验。
- R18. 终态事件只能由显式声明该 topic 的 hat 发布；未声明权限的 hat 发布时必须被拒绝并产生可诊断记录。
- R19. 一个 isolated hat 在同一 backend turn 内发布普通业务事件后，不得通过追加终态事件绕过单业务事件边界。
- R20. 真正由 orchestrator 生成的控制事件必须与 agent 发布的终态事件区分，避免收紧权限后破坏取消、人工交互和恢复流程。

### Fair Isolated Scheduling

- R21. 当多个 isolated hat 同时存在 pending events 时，调度必须保证无饥饿，不能永久偏向字典序最靠前的 hat。
- R22. 调度结果必须具有可测试的确定性；相同 pending 状态和调度历史应产生相同选择。
- R23. 新事件持续回流到同一 hat 时，其他已 pending 的 hat 仍必须在有限轮次内获得执行机会。
- R24. 公平调度不得破坏 aggregate 等待条件、wave worker 调度或直接 target 路由语义。

### Complex Topology Regression Gate

- R25. 必须新增覆盖至少 10 个 hat 的 isolated 端到端集成场景，验证每轮仅执行目标 hat 且事件来源与 topic authority 一致。
- R26. 回归场景必须覆盖线性流、分支汇合、aggregate、wave、失败恢复、human guidance 和合法终止。
- R27. 必须包含非法终态发布场景，证明非授权 hat 无法提前完成 loop。
- R28. 必须包含多个 pending hat 的持续回流场景，证明调度无饥饿且顺序确定。
- R29. 每个迁移后的内置超阈值 preset 必须至少通过配置加载、lint、preflight 和 topology reachability 验证。
- R30. 具有 wave 或 aggregate 行为的迁移 preset 必须通过真实 runtime path 的集成测试，不允许仅用配置文本断言代替。

---

## Key Flows

- F1. 超阈值配置被作者 lint
  - **Trigger:** 作者创建或修改一个包含 4 个及以上 hat 的 coordinator preset。
  - **Actors:** preset author、static lint。
  - **Steps:** lint 计算 hat 配置项总数，识别未显式启用 isolated mode，返回 error finding。
  - **Outcome:** 非法 preset 在进入运行阶段前被拒绝。
  - **Covered by:** R2-R10。

- F2. 非法配置直接运行
  - **Trigger:** 用户绕过独立 lint，直接运行超阈值 coordinator preset。
  - **Actors:** operator、preflight、loop runner。
  - **Steps:** preflight 使用相同规则识别违规，拒绝启动 backend。
  - **Outcome:** loop 不产生部分运行状态，也不自动修改 execution mode。
  - **Covered by:** R2-R9。

- F3. 多个 isolated hat 同时待执行
  - **Trigger:** 分支 workflow 或事件 fan-out 使多个 hat 同时 pending。
  - **Actors:** EventBus、isolated scheduler、target hats。
  - **Steps:** scheduler 基于确定性的公平策略逐个选择 hat；持续回流事件不能垄断执行。
  - **Outcome:** 每个 pending hat 在有限轮次内执行，且每轮只得到自己的指令和事件。
  - **Covered by:** R21-R24、R28。

- F4. 非授权 hat 尝试结束 loop
  - **Trigger:** isolated hat 输出业务事件并追加 completion promise，或直接输出未声明的终态事件。
  - **Actors:** active hat、event scope gate、diagnostics。
  - **Steps:** scope gate 检查终态 authority 和单业务事件边界，拒绝非法事件并记录原因。
  - **Outcome:** workflow 不被提前终止，后续合法 hat 仍可继续执行。
  - **Covered by:** R17-R20、R27。

- F5. 内置 preset 迁移
  - **Trigger:** 发布包含本规则的新版本。
  - **Actors:** preset maintainer、builtin registry、documentation consumers。
  - **Steps:** 超阈值 preset 显式切换 isolated；删除旧 `ce-executor`；同步现行引用；运行复杂 topology 回归门。
  - **Outcome:** 所有现行内置复杂 preset 使用隔离执行，旧入口明确失效。
  - **Covered by:** R11-R16、R25-R30。

---

## Acceptance Examples

- AE1. **Covers R1-R3.**
  - **Given:** 一个包含 3 个 hat 且未声明 execution mode 的 preset。
  - **When:** 执行 lint 和 preflight。
  - **Then:** 不因 multi-hat isolated policy 失败。

- AE2. **Covers R2-R10.**
  - **Given:** 一个包含 4 个 hat 且未声明 execution mode 的 preset。
  - **When:** 执行 lint 或 preflight。
  - **Then:** 两者均以 error 拒绝，并报告实际 hat 数为 4。

- AE3. **Covers R2-R10.**
  - **Given:** 一个包含 4 个 hat 且显式声明 coordinator mode 的 preset。
  - **When:** 执行 lint 或 preflight。
  - **Then:** 仍然失败，显式 coordinator 不能绕过阈值。

- AE4. **Covers R2-R5.**
  - **Given:** 一个包含 8 个 hat，其中 5 个是 aggregate 或 observer 的 preset。
  - **When:** 计算策略阈值。
  - **Then:** hat 数仍为 8，必须使用 isolated mode。

- AE5. **Covers R17-R20.**
  - **Given:** 当前 isolated hat 未声明 completion promise 的发布权限。
  - **When:** backend 输出 completion promise。
  - **Then:** 事件被拒绝、loop 不完成，并产生包含当前 hat 与 topic 的诊断。

- AE6. **Covers R21-R24.**
  - **Given:** hat A 和 hat B 同时 pending，且 hat A 每轮都会收到新的自回流事件。
  - **When:** scheduler 连续选择执行目标。
  - **Then:** hat B 必须在有限轮次内执行，重复运行得到相同调度序列。

- AE7. **Covers R11-R16.**
  - **Given:** 新版本内置 preset registry。
  - **When:** 用户请求 `builtin:ce-executor`。
  - **Then:** 请求明确失败，且 registry 中只存在 `ce-executor-isolated`。

- AE8. **Covers R25-R30.**
  - **Given:** 一个含 10 个以上 hat、wave、aggregate、恢复和人工事件的 isolated fixture。
  - **When:** 通过真实 event loop runtime path 执行。
  - **Then:** 无跨 hat 指令泄漏、无越权发布、无调度饥饿，并只能由授权 hat 合法终止。

---

## Success Criteria

- 所有内置 `hats:` 数量达到 4 的 preset 均显式使用 isolated mode。
- 任意超阈值 coordinator preset 在 lint 和 preflight 阶段均无法通过。
- `builtin:ce-executor` 不再是可解析或可补全的现行 preset。
- 非授权 isolated hat 无法发布 completion promise 或其他受保护终态事件。
- 多 pending hat 压力场景中不存在饥饿，且调度顺序可重复验证。
- 复杂 topology 集成测试通过真实 runtime path 覆盖所有迁移所依赖的关键行为。
- Workspace Rust 测试、replay smoke tests 及相关 preset tests 全部通过。

---

## Scope Boundaries

- 不为超过阈值的 coordinator preset 提供任何豁免。
- 不根据运行时活跃 hat 数、拓扑可达性或 hat 类型动态调整阈值。
- 不在配置加载时自动切换 execution mode。
- 不保留 `ce-executor` 的 alias、兼容映射或 deprecation 执行期。
- 不重写 wave worker 并行模型、aggregate 协议或 EventBus 的发布模型。
- 不把本需求扩展为通用事件溯源或跨 loop 审计系统。
- 不修改历史归档中对 `ce-executor` 的事实性记录。

---

## Dependencies and Assumptions

- isolated mode 继续采用“一次 backend turn 最多推进一个业务阶段”的边界。
- 普通业务事件现有 `publishes` 校验是迁移后的基础安全机制。
- 现有 preset static lint、runtime contract 和 preflight 能承载共享规则定义。
- 历史文档与现行操作文档可以可靠区分；只有现行入口必须清除旧 preset 名称。
- 删除 builtin preset 时需同步维护 preset 单一事实源、公开索引、注册表、项目说明和 shell completion。

---

## Sources

- `crates/ralph-core/src/config/workflow_guards.rs`
- `crates/ralph-core/src/event_loop/mod.rs`
- `crates/ralph-proto/src/event_bus.rs`
- `crates/ralph-core/src/hat_registry.rs`
- `crates/ralph-core/tests/scenarios/isolated_multi_hat.yml`
- `crates/ralph-core/tests/scenarios/isolated_boundary_violation.yml`
- `docs/guide/harness-extensions.md`
- `docs/brainstorms/2026-06-11-ce-executor-hat-impersonation-deep-guard-requirements.md`
