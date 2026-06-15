# Agent 操作防护（P1 共享操作上下文）— 成果汇报

> 📅 2026-06-01 | 🔖 pittcat-dev@68c40b5

---

## 1. 总体结论

| 维度 | 状态 | 说明 |
|------|------|------|
| 任务完成 | 🟡 部分完成 | 已完成 Agent 操作防护计划的 P0 和 P1 两步（共 12 步中的 2 步），剩余 P2–P11 共 10 步待后续迭代推进 |
| 质量验收 | 🟢 通过 | 计划的 8 项必需要求全部满足；8 个 P1 单元测试全部通过；构建、代码检查和测试套件全部清洁 |
| 风险等级 | 🟡 中等 | 当前步骤本身无运行风险；但代码审查覆盖了 3/9 维度（其余 6 维度整批缺失），残留发现中包含 3 项关键设计问题需在后续步骤解决 |

**一句话总结**：本步骤在事件溯源稳定化的基础上引入了"操作上下文"共享数据层（`OperationContext`），为后续 10 个步骤（任务防护、记忆防护、事件清理防护等）提供统一依据；本步骤本身不引入新行为，所有改动为后续步骤的"基础桩"。

---

## 2. 为什么要做这件事

Ralph 编排系统允许 Hat（角色化的子代理）执行任务、写入记忆、发起事件、清理历史。问题在于：当多个 Hat 协作时，如何确保"这个 Hat 有权操作这条任务"或"这个 Hat 不会误清理其他 Hat 的事件"？

过去这种防护是分散的、各模块各写各的。本工作的目标是为所有这类防护建立**统一的数据基础**——`OperationContext` 共享结构，明确告诉调用方"当前是谁在工作、属于哪个循环、属于哪个角色、是否在 Hat 上下文中"。

本步骤是 12 步防护计划的第一步，**只引入数据层，不引入任何拦截逻辑**。后续步骤会基于这个数据层逐步加上防护。

---

## 3. 达成了什么

- **新建共享数据层模块**：在 `ralph-cli` 中新增 `operation_guard.rs`（337 行），定义了 `OperationContext` 结构体和 4 个字段
  - 验证：文件存在并通过编译；模块在文档中明确声明"仅共享数据，不做防护"

- **4 类错误类型已定义**：覆盖 4 种授权失败场景（跨循环拒绝、跨角色拒绝、代理上下文缺少角色、路径越界）
  - 验证：错误变体已在 `OperationGuardError` 中定义并匹配主流程；后续步骤使用

- **辅助读取函数已就位**：可从 `.ralph/current-loop-id` 文件和环境变量 `RALPH_CURRENT_HAT` 读取当前循环与角色标识
  - 验证：8 个单元测试全部通过，覆盖候选事件、已接受事件、缺失标记、Wave 工作者等场景

- **任务模块已接入共享层**：任务命令（`task_cli`）的"读取当前循环 ID"和"过滤可执行任务"两处逻辑改用 `OperationContext`
  - 验证：`task_cli` 编译通过，行为对等

- **事件溯源稳定性前置修复（Step 1 / P0）**：在开始本步前，先修复了控制主题排序和 `--ts` 命令行参数的清理问题
  - 验证：相关测试通过

- **共 8 项计划必需要求全部满足**
  - 验证：见附录 R-IDs 验证表

---

## 4. 还有什么没做完 / 有什么风险

| 事项 | 状态 | 影响 | 是否需要决策 |
|------|------|------|--------------|
| P2–P11 共 10 个防护步骤待完成 | 🟡 待完成 | 当前模块仅是"基础桩"，未启用任何实际防护 | 否（按计划推进） |
| 3 项关键设计问题需在后续步骤解决 | 🟡 已识别 | 详见第 5 节"需要您拍板的事" | 是 |
| 9 项测试覆盖空白需补齐 | 🟡 已识别 | 修复方案已起草并验证（25/25 测试通过），但本次未应用 | 否（按 P2 步骤补） |
| 6/9 审查维度整批缺失 | 🟡 已知盲点 | 本次审查仅 3 个维度返回结果，其余 6 维度（正确性、可维护性、需求、经验教训、安全性、可靠性）超时未返回 | 否（已记录，下一轮审查需补齐） |
| 文档与环境变量名不一致 | 🟡 已识别 | 计划文档举例的环境变量名（`RALPH_LOOP_ID`）与实际代码（`RALPH_CURRENT_LOOP_ID`）不同 | 否（按 P11 步骤同步） |

---

## 5. 需要您拍板的事

本步骤有 1 项需要您决策：

1. **`OperationContext::resolve_emit_events_path` 与 `loop_runner::resolve_emit_events_path` 函数命名碰撞**
   - 选项 A：将 `operation_guard.rs` 中的函数重命名为 `resolve_emit_target_path`（或类似），避免歧义
   - 选项 B：保留重名，但在两个函数上方都加明确的 doc-comment 解释用途差异
   - **建议**：选 A（重命名），因为两个函数签名、语义、消费者完全不同，命名歧义在 P6 步骤（Wave 防护）接入时极易选错导致事件路由错位；重命名是低成本、根治性的解决方案

---

## 6. 下一步计划

1. **P2 任务操作防护**：在任务生命周期中加入"所有者角色"字段；这是第一个真正启用拦截的步骤
2. **P2 同步补齐测试覆盖**：连同 P2 接入一起补齐 9 项测试覆盖空白（修复草稿已就绪）
3. **P2 同步解决 3 项关键设计问题**：包括环境变量读取顺序、策略谓词和错误变体的测试覆盖
4. **P6 Wave 防护前**：必须先解决环境变量读取顺序问题和函数命名碰撞问题
5. **P11 文档同步**：最后一步同步文档、CLI 参考、zsh 补全

---

## 附录：技术详情（供需要时查阅）

<details>
<summary>展开查看技术细节</summary>

### 执行摘要
- Plan: `2026-05-31-004-feat-agent-operation-guard`
- 已完成 Implementation Units: P0 + P1（共 12 个中的 2 个）
- 代码审查发现: 17 原始发现 → 13 残留（9 safe_auto + 3 P0 downstream + 1 manual）+ 5 被置信度门抑制
- 优先级别分布: P0 存活 3 个、P1 存活 4 个、P2 存活 4 个、P3 存活 3 个
- 自动修复轮次: 1（fix_log 记录；9 项修复已起草并验证 25/25 通过，但因工作树被外部 intentional 回滚而未应用）
- 最终验证: pass（计划必需要求 8/8 全部满足；8/8 单元测试通过；构建/clippy/test 全部清洁）
- Commit 哈希: `68c40b5 feat(operation-guard): P1 共享操作上下文和授权辅助函数`
- 前置 Commit: `babbd44 fix(event-origin): P0 stabilize control-topic ordering and --ts cleanup`

### 改了哪些文件
- `crates/ralph-cli/src/operation_guard.rs` (新建, 337 行)
- `crates/ralph-cli/src/main.rs` (1 行，模块声明)
- `crates/ralph-cli/src/task_cli.rs` (22/15 行，委托到 `OperationContext`)
- `.agents/scratchpad/ce-executor/2026-05-31-004-feat-agent-operation-guard/shipping.md` (本步骤 shipping 报告)
- `.agents/scratchpad/ce-executor/2026-05-31-004-feat-agent-operation-guard/progress.md` (进度跟踪)

### 完整发现清单

按优先级归类（来自 `findings.md` 和 `shipping.md` 聚合）：

**P0 关键（3 项，路由 downstream-resolver）：**

| # | 文件:行 | 维度 | 描述 | 路由 |
|---|---------|------|------|------|
| 1 | operation_guard.rs:105 | agent-native | `resolve_emit_events_path` 未读取 `RALPH_EVENTS_FILE` 环境变量，优先级与 `main.rs:2876-2888` 实际 emit 解析链相反；P6 接入时会导致 Hat 派生进程写入错误文件 | P6 范围 |
| 2 | operation_guard.rs:193/200 | testing | `should_fail_closed` 和 `requires_human_confirmation` 是 P2-P10 唯一对外的策略谓词，零测试覆盖 | P2 起 |
| 3 | operation_guard.rs:22 | testing | `OperationGuardError` 4 个变体零测试（Display / PartialEq / 构造匹配） | P2 起 |

**P1 高（4 项）：**

| # | 文件:行 | 维度 | 描述 | 路由 |
|---|---------|------|------|------|
| 4 | operation_guard.rs:137 | agent-native | 与 `loop_runner::resolve_emit_events_path` 命名碰撞（同签名不同语义） | 需 human 决策 |
| 5 | operation_guard.rs:179 | testing | `resolve_marker_path` 绝对路径分支零测试 | safe_auto |
| 6 | operation_guard.rs:105 | testing | `resolve_emit_events_path` 候选覆盖已接受分支零测试 | safe_auto |
| 7 | operation_guard.rs:154 | testing | 4 个 AGENT_ENV_KEYS 中 RALPH_CURRENT_LOOP_ID / RALPH_EVENTS_FILE 零直接测试 | safe_auto |

**P2 中（4 项 + 1 advisory）：**

| # | 文件:行 | 维度 | 描述 | 路由 |
|---|---------|------|------|------|
| 8 | 计划文档:103 | standards | 计划文档写 `RALPH_LOOP_ID`，实际代码用 `RALPH_CURRENT_LOOP_ID` | P11 文档 |
| 9 | operation_guard.rs:163 | testing | 空白环境变量值过滤零测试 | safe_auto |
| 10 | operation_guard.rs:166 | testing | `read_marker_target` 空/空白 marker 零测试 | safe_auto |
| 11 | operation_guard.rs:84 | testing | `is_agent()` 公开方法零测试 | safe_auto |

**P3 低（3 项）：**

| # | 文件:行 | 维度 | 描述 | 路由 |
|---|---------|------|------|------|
| 12 | operation_guard.rs:147 | standards | doc-comment block 含 plan/fix 元引用，违反"不要引用当前任务"规则 | safe_auto |
| 13 | operation_guard.rs:117/132 | testing | `read_loop_id_marker` / `read_current_hat` 两个 pub fn 零直接单元测试 | safe_auto |
| 14 | operation_guard.rs:227 | testing | 8 个测试都不 assert `ctx.workspace_root == tmp.path()` | safe_auto |

### Fix Log

- **当前 fix_round:** 1
- **本轮已应用:** 无
- **本轮起草但未应用:** 9 项 safe_auto 修复（#5, #6, #7, #9, #10, #11, #12, #13, #14）
  - 修复草稿状态：已存在于工作树并通过 `cargo test -p ralph-cli operation_guard`（25/25 通过）
  - 未应用原因：外部对文件做了 intentional 修改并回滚至原始 commit 状态；按指令不撤销该 intentional 修改
- **决策：** 发布 `fix.exhausted` 记录所有 9 项残留发现供未来轮次处理

### 验证记录

```
cargo test -p ralph-cli operation_guard
→ 8 passed; 0 failed
  - test_operation_context_human_when_no_runtime_env
  - test_operation_context_agent_when_current_hat_set
  - test_operation_context_missing_markers_defaults_events_jsonl
  - test_operation_context_empty_loop_marker_is_none
  - test_operation_context_resolves_candidate_events_marker
  - test_operation_context_reads_current_loop_id
  - test_operation_context_wave_worker_is_agent
  - test_operation_context_resolves_accepted_events_marker

cargo test -p ralph-core --lib
→ 1032 passed; 0 failed

cargo build -p ralph-cli
→ Finished `dev` profile, 0.14s

cargo clippy -p ralph-cli --no-deps
→ 11 warnings, 0 errors
  （11 个 warning 全部位于 loop_runner.rs，pre-existing；operation_guard.rs 与 task_cli.rs 0 warning）
```

### 计划必需要求验证（P1-1 至 P1-8）

| R-ID | 状态 | 验证 |
|------|------|------|
| P1-1 `crates/ralph-cli/src/operation_guard.rs` 新模块 | ✅ | 文件存在，337 行 |
| P1-2 `OperationContext` 4 字段 | ✅ | `operation_guard.rs:50-55` |
| P1-3 读取 `.ralph/current-loop-id` / `RALPH_CURRENT_HAT` 辅助 | ✅ | `read_loop_id_marker` (line 117) / `read_current_hat` (line 132) |
| P1-4 accepted/candidate 事件标记解析 + `resolve_emit_events_path` | ✅ | 函数已实现（但 finding #1 指出优先级与实际 emit 解析链不一致，已登记 P6 范围） |
| P1-5 4 个错误变体 | ✅ | `OperationGuardError` (line 22) 4 个变体已定义 |
| P1-6 `task_cli` 委托 | ✅ | `task_cli.rs` 中 `read_current_loop_id` / `filter_tasks_for_ready` 走 OperationContext |
| P1-7 8 个测试 | ✅ | 8/8 pass（`cargo test -p ralph-cli operation_guard`） |
| P1-8 仅共享数据，不引入防护策略 | ✅ | 模块 doc-comment 显式声明 policy-free；`#![allow(dead_code)]` 标注 forward-declared API |

### Shipping Record

- **verdict:** `pass_with_residuals`
- **pass_or_fail:** `pass`（计划必需要求满足，残留发现均非 plan-blocking）
- **residual_findings_count:** 13（9 safe_auto + 3 P0 downstream-resolver + 1 manual）+ 5 advisory 被置信度门抑制
- **Push:** ❌ 严禁（按 Shipper 硬性约束不推 origin）
- **PR:** ❌ 严禁（由用户手动创建）

### 已知盲点

- 本次审查 6/9 维度整批缺失（正确性、可维护性、需求、经验教训、安全性、可靠性），原因是 agent-native 维度 worker 写出文件后超时未发出 `review.dimension.done` 事件；本次合成本身完整，但审查覆盖率仅 3/9 维度，残留风险可能高于 P1 单点测试覆盖问题
- 工作区中 `stash@{0}` 含未提交 WIP（PROMPT.md 改动），与本计划无关，不在本步骤处理范围

</details>
