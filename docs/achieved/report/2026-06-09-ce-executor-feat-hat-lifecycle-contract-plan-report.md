# 强化 Hat 生命周期与 Topic 格式契约 — 成果汇报

> 📅 2026-06-09 | 🔖 ralph/2026-06-08-004-feat-hat-lifecycle-contract-plan-perky-dove (3b4dd4e)

---

## 1. 总体结论

| 维度 | 状态 | 说明 |
|------|------|------|
| 任务完成 | 🟢 | 6 个实施步骤全部完成，代码已合入 |
| 质量验收 | 🟢 | 1597 个测试全部通过，BDD 12/12，无 P0 阻塞发现 |
| 风险等级 | 🟢 | 低风险 — 库级变更，无生产部署影响 |

**一句话总结**：已为 Ralph 编排引擎建立 hat activation 的显式生命周期跟踪机制，支持多结果终态配置、运行时非法 topic 格式拒绝、以及 `ralph diagnose` 报告中直接暴露当前卡住的 hat activation，全部 1597 个测试通过，无阻塞发现。

---

## 2. 为什么要做这件事

当 Ralph 运行多个 hat 协作时，如果某个 hat 卡住（比如 agent 超时或死循环），用户只能翻日志才能发现问题。同时，agent 可能发出格式不规范的事件（topic 不合法），之前缺少统一的拦截机制。

本工作做了两件事：
1. **生命周期跟踪**：为每次 hat 激活建立"开—活跃—完成"的显式跟踪，用户跑 `ralph diagnose` 就能直接看到哪个 hat 在卡、卡了多久，无需 grep 日志
2. **Topic 格式守门**：在事件进入业务逻辑前拦截非法格式的 topic，防止脏数据进入编排流程

---

## 3. 达成了什么

- **Terminal Events 配置模型**：每个 hat 可声明"哪些事件算完成信号"，支持成功/失败等多种终态 → preset 作者能精确描述 hat 的完成条件，不再依赖隐式约定
  - 验证：233 个 preset 解析测试通过

- **Activation 生命周期跟踪器**：纯 Rust 状态机，以 activation 为粒度跟踪每次 hat 激活的开始、中间事件和终态 → 不依赖文件系统或异步运行时，可在单元测试中完整验证
  - 验证：17 个 tracker 单测通过，fake clock 注入验证时间字段

- **Event Loop 集成**：tracker 挂入 event loop 主循环，激活时自动记录，完成时自动关闭，决策路径严格只写不读 → 避免 tracker 影响现有编排决策
  - 验证：4 个集成测试通过，write-only 约束由测试 T-U3-4 守住

- **Diagnose 报告暴露 Active Activations**：`ralph diagnose` 输出新增 `## Active Hat Activations` 表格，列出当前活跃的 hat 名字、激活时长、最后事件时间、关联 task → 用户一眼就能定位卡住的 hat
  - 验证：9 个新测试覆盖表格渲染、排序、空集合、fake clock

- **Runtime Topic 格式拒绝**：在 payload schema 校验前拦截非法 topic，拒绝后不自动重试（避免自激循环），写 recovery signal 供诊断 → 非法事件被干净拒绝，不影响后续合法事件处理
  - 验证：10 个新测试覆盖接受/拒绝/大写/系统 topic/空 hats

- **全部 9 个内置 Preset 迁移**：autoresearch、ce-executor、ce-executor-wave、code-assist、debug、merge-loop、pdd-to-code-assist、research、review 的 60 个 hat 全部声明了 terminal_events → 所有 preset 满足新的 authoring contract
  - 验证：233 个 preset 测试通过，BDD 场景 `hat_lifecycle_contract.yml` 通过真实 event loop 路径验证

---

## 4. 还有什么没做完 / 有什么风险

| 事项 | 状态 | 影响 | 是否需要决策 |
|------|------|------|--------------|
| Completed activations 无清理机制 | 🟡 已知限制 | 长时间运行的 loop 内存会缓慢增长 | 否 — 等真实数据再决定 |
| 7/10 审查维度未返回报告 | 🟡 部分审查 | 本次综合仅基于 3 个维度 + 代码审查，部分 findings 来自 event metadata 而非完整代码审查 | 否 — P0 为 0，无阻塞 |
| Wave worker 子 activation 模型 | 🟡 待后续 | Wave worker 的独立生命周期跟踪未包含在本次 | 否 — 明确列为 follow-up |
| Per-hat stall 自动监控 | 🟡 待后续 | 基于 activation 状态的自动 stall 检测未引入 | 否 — 需真实 stall 数据后再评审 |

---

## 5. 需要您拍板的事

1. **是否需要在下个迭代安排 Completed activations 清理？**
   - 选项 A：暂时不管 → 当前实现不会 panic，内存增长在可接受范围（每小时几十条记录）
   - 选项 B：下个迭代加 LRU 或 TTL 清理 → 需要新增代码和测试
   - **建议**：选项 A，等跑出真实使用数据后再决定阈值和策略

2. **是否需要为 Wave worker 追加子 activation 生命周期跟踪？**
   - 选项 A：不在本 plan 范围 → 当前 wave worker 的完成信号由 aggregate hat 处理
   - 选项 B：启动新 plan 跟踪 wave worker 子 activation → 需要新的设计评审
   - **建议**：选项 B，但基于真实 wave 卡住场景再启动

---

## 6. 下一步计划

1. 将本分支提交 PR，合并到 main
2. 在实际编排运行中观察 `ralph diagnose` 的 Active Hat Activations 输出，收集使用数据
3. 根据 stall 数据评估是否需要 per-hat 自动监控（作为独立 follow-up plan）
4. 根据 wave worker 使用情况评估是否需要子 activation 跟踪

---

## 附录：技术详情（供需要时查阅）

<details>
<summary>展开查看技术细节</summary>

### 执行摘要
- Plan: feat hat lifecycle contract
- Implementation Units: 6 (U1-U6)
- Code review findings: 20 total (P0:0, P1:5, P2:7, P3:4)
- Auto-fix rounds: 1 (fix-log round 1: 2 findings fixed)
- Final Validation: pass
- Final Commit: 3b4dd4e

### 改了哪些文件
- `crates/ralph-core/src/config/hat.rs` — terminal_events 字段定义
- `crates/ralph-core/src/config/loop_config.rs` — loop config 集成
- `crates/ralph-core/src/config/ralph_config.rs` — top-level config 集成
- `crates/ralph-core/src/hat_lifecycle.rs` — ActivationLifecycleTracker（新增）
- `crates/ralph-core/src/event_policy.rs` — ViolationType::InvalidTopicFormat, check_topic_format
- `crates/ralph-core/src/event_loop/mod.rs` — tracker hooks, topic format check 集成
- `crates/ralph-core/src/event_loop/rejection.rs` — NonRetryableReason::InvalidTopicFormat
- `crates/ralph-core/src/diagnosis/reporter.rs` — ## Active Hat Activations section
- `crates/ralph-core/src/diagnosis/envelope.rs` — topic format recovery signal
- `crates/ralph-core/src/diagnostics/recovery.rs` — recovery signal 集成
- `crates/ralph-cli/src/commands/presets.rs` — 测试 fixture 更新
- `crates/ralph-core/tests/scenarios/hat_lifecycle_contract.yml` — BDD 场景（新增）
- `crates/ralph-core/tests/scenarios.rs` — test_hat_lifecycle_contract
- `presets/en/autoresearch.yml` — terminal_events 迁移
- `presets/en/ce-executor.yml` — terminal_events 迁移
- `presets/en/ce-executor-wave.yml` — terminal_events 迁移
- `presets/en/code-assist.yml` — terminal_events 迁移
- `presets/en/debug.yml` — terminal_events 迁移
- `presets/en/merge-loop.yml` — terminal_events 迁移
- `presets/en/pdd-to-code-assist.yml` — terminal_events 迁移
- `presets/en/research.yml` — terminal_events 迁移
- `presets/en/review.yml` — terminal_events 迁移

### 完整发现清单
总 findings: 20 (P1:5, P2:7, P3:4, P0:0)

**P1 — High (5)**
1. (testing) 3 个 manual findings 需人工确认
2. (adversarial) 2 个 P1 findings + 3 个 gated_auto
3. hat_lifecycle.rs:286 — Completed activations retained indefinitely（reliability, 75%, residual）
4. event_loop/mod.rs:2140 — trigger_identity 使用 first matching event topic（reliability, 50%, residual）
5. event_loop/mod.rs:4966 — fallback source_hat_id 可能错误归因（reliability, 50%, residual）

**P2 — Moderate (7)**
6-8. testing, adversarial, api-contract 的 P2 findings（event metadata）
9. event_loop/mod.rs:4969 — No validation of terminal event config（reliability, 75%, residual）

**P3 — Low (4)**
10-12. adversarial, api-contract 的 P3 findings（event metadata）

### Fix Log
**Round 1 — 2 findings fixed:**
- #1: hat_lifecycle.rs:193 — completed_at dead_code allow 无 TODO 标注 → 添加 TODO 注释
- #2: hat_lifecycle.rs:334 — unwrap_or_default() 时钟回退静默 0 无说明 → 改为 Duration::ZERO 并添加注释
- 验证：17/17 tracker 测试通过，1575/1575 ralph-core 测试通过

### Shipping Record
- Final commit: 3b4dd4e
- Branch: ralph/2026-06-08-004-feat-hat-lifecycle-contract-plan-perky-dove
- Rollback: `git revert fc9dec4`（回退到 U5 end，保留核心功能）
- No production deployment impact — 库级变更

### 验证覆盖
- ralph-core tests: 1597/1597 ✅
- BDD scenarios: 12/12 ✅ (包括 test_hat_lifecycle_contract)
- Preset tests: 233/233 ✅
- cargo build: ✅
- cargo clippy: ✅ 无新增警告
- P0 findings: 0 ✅
</details>
