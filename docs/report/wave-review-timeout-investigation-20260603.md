# Wave Review 超时问题调查报告

> 调查时间：2026-06-03
> 调查对象：两个 worktree 的 wave review 执行记录
> Worktree A: `implement-dev-plan-docs-plans-peppy-willow` (plan: `2026-06-02-005-feat-payload-contract-validation-plan`)
> Worktree B: `implement-dev-plan-docs-plans-sharp-birch` (plan: `2026-06-03-001-feat-agent-execution-contract-gates-plan`)

---

## 1. 问题概述

两个 worktree 均在 wave review 阶段出现大量 worker 超时：

| Worktree | Plan | Wave ID | 总 worker 数 | 成功 | 超时 | Loop 终止原因 |
|----------|------|---------|-------------|------|------|--------------|
| peppy-willow | payload-contract-validation | w-18b5567bc50c4120-14410-0 | 13 | 2 (correctness, learnings) | 10+ | max_runtime (4h22m) |
| sharp-birch | agent-execution-contract-gates | w-18b55fb629ff8b58-19832-0 | 7 | 1 (correctness, 217s) | 5 | max_runtime (4h21m) |

**共同模式**：correctness 维度总是最快完成，其他维度普遍超时。

---

## 2. 根因分析

### 2.1 核心问题：dimension-reviewer 指令过于复杂

每个 dimension-reviewer worker 收到的指令包含：

1. **主体指令**：完整的 hat 角色定义（`## DIMENSION REVIEWER MODE`）
2. **Wave Context**：worker 身份和任务说明
3. **Task Payload**：dimension-specific focus（400-600 字符的详细检查项）
4. **Findings Schema**：完整的 JSON 输出规范
5. **Dimension Checklists**：7-11 个维度的完整检查清单（每个 80-200 字符）
6. **Publishing Guide**：如何通过 `ralph emit` 发布结果

**总指令量估计**：单 worker 收到的完整 prompt 超过 3000 token。

### 2.2 问题维度差异分析

| 维度 | peppy-willow | sharp-birch | 可能原因 |
|------|--------------|-------------|---------|
| correctness | ✓ 成功 | ✓ 成功 (217s) | 检查项明确（逻辑错误、边界条件），可通过代码推理快速完成 |
| testing | ✓ 成功 | — 超时 | 需要运行测试（cargo test），验证时间长 |
| maintainability | ✓ 成功 | — 超时 | 结构性检查，代码量已知 |
| standards | ✓ 成功 | — 超时 | 需要读取 CLAUDE.md/AGENTS.md，跨文件分析 |
| requirements | — 超时 | — 超时 | 需要读取 plan.md/context.md，验证 R-ID |
| agent-native | — 超时 | — 超时 | 需要理解 agent 工具可访问性 |
| learnings | ✓ 成功 | — 超时 | 需要搜索 docs/solutions/，跨时间分析 |

### 2.3 Diff 规模对比

| Worktree | Diff 行数 | 涉及文件数 | 复杂度 |
|----------|-----------|-----------|--------|
| peppy-willow | 1126 行 | 4 个 Rust 文件 | 中等 |
| sharp-birch | 631 行 | 6 个 preset YAML + Rust | 中等 |

对于需要「交叉引用」「对比分析」的维度（如 learnings、requirements），diff 规模放大了分析难度。

### 2.4 Event Publishing 路径问题

Dimension-reviewer 指令要求：
```
- Use the bash tool to execute: `ralph emit review.dimension.done --json '<payload>'`
```

**在 ACP 模式下**，wave worker 通过 `AcpExecutor` 与 Claude Code ACP 协议通信。当 worker 尝试执行 `ralph emit` bash 命令时：

1. ACP 协议将 bash tool call 发送回 orchestrator
2. Orchestrator 执行 `ralph emit`
3. 事件写入 worker 的 `worker_events_file`（`wave-{wave_id}-{index}.jsonl`）

**潜在问题**：
- `read_worker_events()` 只在 `run_wave_worker_acp` 返回 **后** 才被调用
- 如果 worker 超时但实际上已经写入了事件，这些事件在 timeout 分支中**被忽略**
- 事件只在 `AcpWaveExecutionResult::Completed(Ok(success))` 分支中被读取

```rust
// loop_runner.rs:6560
let events = read_worker_events(worker_events_path);

match result {
    AcpWaveExecutionResult::Completed(Ok(success)) => {
        // events 在这里被使用 ✓
        (index, Ok((events, duration, success)))
    }
    AcpWaveExecutionResult::TimedOut if events.is_empty() => {
        // events.is_empty() 时才报告失败
    }
    AcpWaveExecutionResult::TimedOut => {
        // events 非空但被丢弃！worker 可能已经写入了 events
        (index, Ok((events, duration, false)))
    }
}
```

### 2.5 Concurrency 与 Aggregate Timeout 计算

```yaml
dimension-reviewer:
  concurrency: 4
  timeout: 900
```

对于 7 个 workers（sharp-birch）：
- 理论并发：4 个同时运行
- Batch 数：ceil(7/4) = 2
- **Per-worker timeout**：900s（硬性限制）
- **Aggregate timeout**：2 × 900s + 30s buffer = 1830s (30.5 分钟)

**时序分析（sharp-birch）**：

```
21:07:43 - review-coordinator 发射 7 个 wave events（wave_total=7）
21:07:43 - Workers 0,1,2,3 立即启动（第 1 批，concurrency=4）
21:07:43 - Workers 4,5,6 等待 semaphore（第 2 批）
21:12:56 - Worker 0 (correctness) 在 ~317s 完成
21:12:56 - Worker 4 获得 permit，启动（开始计时）
21:34:xx - Workers 1,2,3 在 ~900s 时陆续超时
21:34:xx - Worker 4 已经运行了约 821s（21:12:56 启动）
21:38:24 - Loop 达到 max_runtime (4h21m)，整体终止
```

对于 13 个 workers（peppy-willow）：
- 理论并发：4 个同时运行
- Batch 数：ceil(13/4) = 4
- **Per-worker timeout**：900s（硬性限制）
- **Aggregate timeout**：4 × 900s + 30s buffer = 3630s (60.5 分钟)

---

## 3. 预设配置分析

### 3.1 ce-executor.yml 中的 dimension-reviewer 配置

```yaml
dimension-reviewer:
  name: "🔬 Dimension Reviewer"
  description: "Focused wave-based code review for a single dimension."
  triggers: ["review.wave.ready"]
  publishes: ["review.dimension.done"]
  concurrency: 4
  timeout: 900
```

**问题**：
1. `concurrency: 4` 限制了同时运行的 worker 数量，但 wave_total=7/13 意味着有大量 worker 需要等待
2. `timeout: 900` 对某些维度**不足够**，尤其是：
   - `learnings`：需要搜索 docs/solutions/ 历史记录
   - `requirements`：需要读取多个文件交叉验证
   - `testing`：可能需要运行测试验证

### 3.2 review-coordinator 的 wave emission

Review-coordinator 为每个维度发射独立的 wave event（7-13 个维度），每个 event 的 payload 包含：
- `dimension`: 维度名称
- `focus`: 详细的检查说明（400-600 字符）
- `depth`: 审查深度
- `diff_base`, `intent_summary`, `changed_files`
- `task_id`, `task_key`, `step`

这些 payload 信息会被注入到每个 worker 的 prompt 中。

---

## 4. 两 Worktree 对比分析

### 4.1 peppy-willow（13 workers，4 并发）

**优势**：
- correctness, learnings, maintainability, testing 四个维度成功完成
- 说明 Ralph 机制本身可以正常工作

**问题**：
- 9 个维度超时（standards, requirements, agent-native, adversarial 等）
- 13 个 workers 的 aggregate timeout 为 3630s（约 1 小时），但 loop max_runtime 只有 4.5 小时

### 4.2 sharp-birch（7 workers，4 并发）

**优势**：
- correctness 维度在 217s 成功完成
- 说明基本机制正常

**问题**：
- 5 个维度超时
- 耗时最长只到 900s（timeout 限制），没有 worker 能坚持到 aggregate timeout

### 4.3 共同结论

**核心问题不在 Ralph 机制，而在预设指令设计**：

1. 指令过于复杂，单 worker 需要处理 3000+ token
2. Focus payload 过长（400-600 字符），增加了处理时间
3. Timeout 对某些维度（如 learnings、requirements）不足
4. 所有维度同时发射，没有根据复杂度分级

---

## 5. 结论

### 5.1 是 ce-executor 预设问题还是 ralph 机制问题？

**结论：主要是 ce-executor 预设的指令设计问题，RALPH 机制本身基本正常。**

**RALPH 机制正常运行的证据**：
- Wave 检测和分发正确
- Worker 超时机制按预期工作（900s timeout）
- 成功的 workers 正确完成并发布事件
- Review-synthesizer 的 `wait_for_all` 聚合逻辑正确处理了部分超时
- 并发限制（concurrency: 4）正确执行

**ce-executor 预设的问题**：

| 问题 | 严重程度 | 说明 |
|------|----------|------|
| 指令过于复杂 | P1 | 单 worker 3000+ token，包含大量检查清单 |
| Focus 字符串过长 | P1 | 400-600 字符的详细指令增加了 worker 处理时间 |
| Timeout 对某些维度不足 | P2 | learnings/requirements 维度需要更多时间 |
| Diff 规模与审查时间不匹配 | P2 | 631-1126 行 diff 对 7-13 个并行维度是合理的，但某些维度需要更长时间 |
| Wave 发射策略不够智能 | P2 | 同时发射所有维度，没有根据复杂度分级 |

### 5.2 建议修复方向

#### 短期（立即可做）：
1. **增加 dimension-reviewer timeout**：从 900s 提升到 1800s（30 分钟）
2. **精简 dimension-reviewer 指令**：将检查清单移到外部文件，worker prompt 只保留核心指令
3. **优化 focus payload**：缩短到 200 字符以内，只描述"查什么"，不描述"怎么查"

#### 中期（需要测试）：
1. **分阶段 wave 发射**：先发 correctness + testing（最快），再发其他维度
2. **动态 timeout**：根据 diff 规模动态计算 timeout
3. **Worker 进度检查**：在 timeout 之前检查 worker 是否有进展

#### 长期（架构优化）：
1. **Review 任务持久化**：worker 不需要在单个 prompt 中完成所有工作
2. **增量式 review**：分多次迭代完成，而非单次大规模审查
3. **专门的 review 模式**：与普通 agent 不同的执行策略

---

## 6. 附录：关键代码位置

| 文件 | 行号 | 说明 |
|------|------|------|
| `crates/ralph-core/src/wave_detection.rs` | 28-42 | `timeout_secs()` 计算逻辑 |
| `crates/ralph-cli/src/loop_runner.rs` | 6034-6300 | `execute_wave()` 主函数 |
| `crates/ralph-cli/src/loop_runner.rs` | 6538-6580 | `run_wave_worker_acp()` 事件读取逻辑 |
| `crates/ralph-cli/src/loop_runner.rs` | 6498-6535 | `execute_wave_worker_acp_prompt()` ACP 执行 |
| `crates/ralph-adapters/src/acp_executor.rs` | 417-500 | `AcpExecutor::execute()` ACP 协议处理 |
| `presets/ce-executor.yml` | 376-382 | dimension-reviewer hat 配置 |
| `presets/ce-executor.yml` | 430-570 | dimension-reviewer 完整指令 |

---

## 7. 附录：peppy-willow 事件时间线

```
18:18:37 - review-coordinator 发射 13 个 wave events
18:18:37 - Workers 0,1,2,3 立即启动（第 1 批，concurrency=4）
18:xx:xx - Workers 4,5,6 等待 semaphore（第 2 批）
18:xx:xx - Workers 7,8,9 等待 semaphore（第 3 批）
18:xx:xx - Workers 10,11,12 等待 semaphore（第 4 批）
~18:40 - correctness 完成
~18:43 - testing 完成
~19:01 - learnings 完成
~19:10 - maintainability 完成
后续 - 其他维度陆续超时
max_runtime - Loop 终止
```

---

## 8. 附录：sharp-birch 事件时间线

```
21:07:43 - review-coordinator 发射 7 个 wave events
21:07:43 - Workers 0,1,2,3 立即启动（第 1 批，concurrency=4）
21:07:43 - Workers 4,5,6 等待 semaphore（第 2 批）
21:12:56 - Worker 0 (correctness) 完成，耗时 317s
21:12:56 - Worker 4 获得 permit，启动（开始计时）
21:34:xx - Workers 1,2,3 在 ~900s 时陆续超时
21:34:xx - Worker 4 已经运行了约 821s（21:12:56 启动）
21:38:24 - Loop 达到 max_runtime (4h21m)，整体终止
```
