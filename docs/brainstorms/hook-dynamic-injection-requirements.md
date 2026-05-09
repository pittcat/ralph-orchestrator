# 动态 Hook 注入与编排需求文档

## 1. 背景与动机

当前 Ralph Orchestrator 和 Claude Code 的 hooks 系统均为**静态配置驱动**：所有 hook 必须在启动前通过配置文件（`ralph.yml` 或 `.claude/settings.json`）声明，运行期间无法新增、移除或调整 hook。这在以下场景中存在明显局限：

- 想要根据 loop 运行中的实际状态（如某次 iteration 产生了关键决策），临时触发一个"记笔记"或"总结"动作
- 想要在不同阶段灵活组合行为，而非在配置文件中写死一套固定流程
- 想要让 agent（hat）自身决定"我现在需要调用什么外部工具/逻辑"

因此，需要探索一种**运行时动态注入**机制，使 hooks 从"静态编排"升级为"动态编排"。

---

## 2. 核心需求

### 2.1 动态 Hook 注册（Dynamic Hook Registration）

| ID | 需求描述 | 优先级 |
|---|---|---|
| DH-01 | 在 loop 运行期间，能够向指定生命周期阶段动态注册一个 hook | P0 |
| DH-02 | 动态注册的 hook 应支持一次性执行（执行后自动移除）或持久驻留 | P1 |
| DH-03 | 能够动态移除已注册的 hook（无论是静态配置的还是动态注入的） | P1 |
| DH-04 | 动态 hook 的注册/移除操作本身应该可通过某个接口触发（如事件、命令、或 agent 输出） | P1 |

### 2.2 Hook 编排（Hook Orchestration）

| ID | 需求描述 | 优先级 |
|---|---|---|
| HO-01 | 支持在运行时动态调整某阶段 hook 的执行顺序 | P1 |
| HO-02 | 支持条件触发：仅当 payload 满足某条件时才执行某个 hook | P1 |
| HO-03 | 支持 hook 链（chaining）：hook A 的输出可以作为 hook B 的输入 | P2 |
| HO-04 | 支持并行执行同一阶段的多个 hook（当前 Ralph 是串行） | P2 |

### 2.3 具体功能场景（Use Cases）

| ID | 场景 | 期望行为 |
|---|---|---|
| UC-01 | 自动记笔记 | 每次 iteration 结束后，自动提取关键决策/变更并写入 `.ralph/agent/notes.md` |
| UC-02 | 智能总结 | 当 loop 即将完成或检测到阶段转换时，触发总结逻辑生成摘要 |
| UC-03 | 动态质量门 | 根据当前代码变更内容，动态决定是否插入 lint/test hook |
| UC-04 | 上下文感知通知 | 当 human.interact 事件触发时，根据问题类型动态选择通知渠道 |
| UC-05 | 运行时诊断注入 | 当检测到异常行为时，动态注入诊断 hook 收集调试信息 |

---

## 3. 当前状态分析

### 3.1 Ralph Orchestrator Hooks

```
配置位置：ralph.yml → hooks.events
引擎位置：crates/ralph-core/src/hooks/engine.rs
执行位置：crates/ralph-cli/src/loop_runner.rs
```

**现状约束：**
- `HookEngine::new()` 启动时从 `HooksConfig` 克隆所有 hooks 到 `HashMap`，运行期不可变
- `dispatch_phase_event_hooks()` 按声明顺序串行执行，无并行、无条件过滤、无链式传递
- Hook 之间仅通过 `accumulated_hook_metadata` 传递 JSON 状态，无直接的输入输出管道
- Hook 脚本是外部进程，通过 stdin 接收 payload，stdout/stderr 被捕获

**现有能力可复用：**
- `HookRunRequest` / `HookRunResult` 执行契约
- `HookInvocationPayload` JSON schema（含 loop/iteration/context/metadata）
- `accumulated_hook_metadata` 状态传递机制
- `mutate` 配置对事件循环的修改能力

### 3.2 Claude Code Hooks

```
配置位置：.claude/settings.json
类型：user-prompt-submit-hook / pre-tool-use-hook / post-tool-use-hook / session-start-hook
```

**现状约束：**
- 同样是启动前静态配置
- 每个 hook 是外部命令，通过环境变量接收上下文
- 无运行时 API 可动态增删

---

## 4. 待探索的关键问题

### 4.1 触发接口设计

动态注入的触发源有哪些候选？

| 方案 | 说明 | 复杂度 |
|---|---|---|
| A. 事件总线扩展 | agent 在事件中声明需要注册的 hook，loop_runner 解析并执行 | 中 |
| B. 特殊 stdout 协议 | hook 脚本输出约定格式的 JSON（如 `{"ralph.hook.register": {...}}`），loop_runner 解析 | 低 |
| C. 文件系统监视 | 将动态 hook 定义写入 `.ralph/dynamic-hooks/`，loop_runner 定时扫描 | 中 |
| D. RPC/API 接口 | `ralph-api` 暴露注册/移除 endpoint，外部调用 | 高 |
| E. Agent 自然语言指令 | agent 在回复中写"请帮我注册一个 hook 来做 X"，loop_runner 用 LLM 解析 | 高 |

### 4.2 作用域与生命周期

- 动态注册的 hook 是全局生效还是仅对当前 loop 生效？
- 是否区分"主 loop"和"worktree loop"的作用域？
- loop 重启后动态 hook 是否保留？

### 4.3 安全与隔离

- 动态注入是否需要有权限控制（防止 agent 滥用）？
- 是否限制动态 hook 只能调用白名单内的命令？
- 是否需要审计日志记录所有动态注册/移除操作？

### 4.4 与现有静态 Hook 的关系

- 动态 hook 和静态 hook 的执行顺序如何定义？
- 动态 hook 能否访问/修改 `accumulated_hook_metadata`？
- 是否允许动态 hook 覆盖同名的静态 hook？

---

## 5. 可能的实现方向

### 方向一：最小侵入式扩展（Minimal Extension）

仅扩展 `HookEngine`，增加 `register/unregister` 方法，并在每次 `dispatch_phase_event_hooks` 前重新解析 hooks 列表。

**改动点：**
- `HookEngine` 内部用 `RwLock<HashMap<...>>` 替代纯不可变结构
- `loop_runner` 在每次 dispatch 前检查是否有新的动态注册
- 新增一个内部事件类型（如 `DynamicHookRegister`）供 agent 触发

**优点：** 改动小，兼容现有静态配置
**缺点：** 每次 dispatch 有锁开销（可忽略）

### 方向二：Hook 即服务（Hook-as-a-Service）

将 hooks 抽象为独立的服务/插件，通过事件总线解耦。Ralph 本身只负责调度，hook 逻辑由外部服务实现。

**改动点：**
- 定义 `HookService` trait（注册、执行、状态查询）
- 内置一个 `ScriptHookService`（当前行为）
- 允许 agent 通过事件注册新的 `HookService` 实例

**优点：** 架构清晰，易于扩展
**缺点：** 改动较大，需要重构 hooks 模块

### 方向三：利用现有 Metadata 通道

不改动 hooks 架构，而是扩展 `accumulated_hook_metadata` 的语义，让 agent 通过 metadata 声明"虚拟 hook"，由一个统一的 dispatcher hook 根据 metadata 路由执行。

**改动点：**
- 增加一个常驻的 `dynamic-dispatcher` hook
- agent 在事件中写入 `{"_dynamic_hooks": [{"phase": "...", "action": "..."}]}`
- dispatcher 解析 metadata 并调用对应脚本

**优点：** 零架构改动，纯应用层方案
**缺点：** 不够优雅，所有动态逻辑挤在一个 hook 里

---

## 6. 下一步建议

1. **明确触发方式**：你更倾向于哪种触发接口？是 agent 主动声明、还是通过某种协议自动解析？
2. **确定 MVP 场景**：从 UC-01~UC-05 中挑选 1~2 个最高优先级场景做 POC
3. **选择实现方向**：方向一（最小侵入）vs 方向三（纯 metadata）适合快速验证
4. **产出设计文档**：如果方向确定，可以产出详细的技术设计文档（TDD）进入实现阶段

---

*文档版本: v1.0*
*日期: 2026-05-07*
*状态: 需求草稿，待评审确认*
