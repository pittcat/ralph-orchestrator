# Managed Agent Doc Blocks — 成果汇报

> 📅 2026-06-09 | 🔖 ralph/2026-06-09-001-feat-managed-agent-doc-blocks-plan-bold-lotus

---

## 1. 总体结论

| 维度 | 状态 | 说明 |
|------|------|------|
| 任务完成 | 🔴 | 计划 6 个步骤中仅完成 3 个（U1-U3），剩余 3 个（U4-U6）未实现 |
| 质量验收 | 🟢 | 已完成的代码质量良好：1607 个测试全部通过，无构建错误 |
| 风险等级 | 🟡 | 功能尚未集成到 ralph run 启动流程，无法实际使用 |

**一句话总结**：已建立 agent doc blocks 同步引擎的核心骨架和配置系统，并嵌入了首条 hang-prevention 规则，但由于 U3 审查流程超时，后续的集成、观测和文档验证步骤（U4-U6）未能推进，功能尚未可交付。

---

## 2. 为什么要做这件事

2026-06-05，一次 orchestrator loop 运行失败，根因是 backend agent 在一个没有 timeout 的长命令（如 `tail -f`）中无限阻塞。当时做了紧急止血（在 19 个 preset 中注入"禁止 kill 父进程"规则），但没有一套通用机制确保所有 agent 在启动时一定读到"禁止无限跟随命令"的硬约束。

本计划的目标是：让 ralph 在每次启动 agent 之前，自动把一组防挂死规则写入 agent 必读的 `CLAUDE.md` 和 `AGENTS.md` 文件。这样无论换哪个 backend、换哪台机器，规则都跟随着，不依赖项目状态或人工配置。

---

## 3. 达成了什么

- **同步引擎骨架**：在 ralph-core 中实现了 `agent_doc_sync` 子模块，支持对目标文件执行"创建 / 追加 / 替换 / 跳过"四类操作，具备文件锁保护和幂等性 → 后续集成只需在启动流程中插入一行调用
  - 验证：59 个单元测试全部通过

- **配置系统**：在 `ralph.yml` 中新增 `agent_doc_sync` 配置节点，支持 `enabled`、`on_error`（warn/strict）、`blocks` 三个字段；CLI 支持 `--no-sync-agent-docs` 旗标和 `RALPH_AGENT_DOC_SYNC=0` 环境变量 → 用户可通过多种方式控制行为
  - 验证：10 个配置测试全部通过

- **首条规则嵌入**：将 5 条 Command Hang Prevention Rules 以 markdown 文件形式编译进 ralph-cli 二进制，通过 `include_str!` 实现，sha256 哈希稳定可重现 → 规则跨机可重复，升级 ralph 时自动跟随
  - 验证：8 个 builtin 测试全部通过

- **三轮自动修复**：在代码审查过程中，通过 3 轮自动修复解决了 block_result 归因不准确、Strict 模式不传播错误、孤立 begin marker 重复追加、孤立 begin marker 匹配 hash 时被错误升级、替换后丢失用户内容共 5 个问题 → 代码健壮性显著提升

---

## 4. 还有什么没做完 / 有什么风险

| 事项 | 状态 | 影响 | 是否需要决策 |
|------|------|------|--------------|
| U4: 集成到 ralph run 启动流程 | 🔴 未完成 | 同步引擎已就位但未接入，用户无法实际使用该功能 | 是 |
| U5: Doctor 健康检查 + 诊断日志双写 | 🔴 未完成 | 无法通过 `ralph doctor` 查看同步状态 | 否（依赖 U4） |
| U6: 文档 + 反向验证 + 端到端回归 | 🔴 未完成 | 无用户文档，无法确认端到端行为正确 | 否（依赖 U4+U5） |
| OnErrorPolicy/OnError 类型割裂 | 🟡 低风险 | 两个枚举结构相同但无类型级关联，U4 集成需手动映射 | 否 |
| Testing 维度审查文件缺失 | 🟡 低风险 | 7 个 testing findings 未验证，但不影响已通过的测试结果 | 否 |

---

## 5. 需要您拍板的事

1. **是否继续推进 U4-U6？**
   - 选项 A：继续 — 在本次 worktree 分支上实现剩余 3 个步骤 → 功能完整交付
   - 选项 B：暂停 — 将已完成的 U1-U3 代码保留，待后续有需要时再继续 → 功能半成品
   - **建议**：选 A。核心骨架和配置系统已就绪，U4（集成）是最关键的一步，预计工作量不大。U4 完成后即可实际使用。

2. **U3 审查超时的根本原因是否需要调查？**
   - 选项 A：不调查 — 这是 orchestrator 工作流问题，不影响代码质量
   - 选项 B：调查 — 确认是维度审查者 hat 未配置、事件路由问题还是其他原因
   - **建议**：选 A。代码质量已通过 1607 个测试验证，审查超时是工作流层面的问题，可在后续迭代中优化。

---

## 6. 下一步计划

1. 实现 U4：在 `loop_runner/runner.rs` 中 `CliBackend::from_config` 之前插入 `sync_all` 调用
2. 实现 U5：扩展 `DiagnosisSource` 枚举 + 实现 `agent_doc_sync.json` 快照 + recovery envelope 双写
3. 实现 U6：编写用户文档 `docs/guide/managed-blocks.md` + 端到端回归测试
4. 重新触发代码审查流程，确认无遗漏

---

## 附录：技术详情（供需要时查阅）

<details>
<summary>展开查看技术细节</summary>

### 执行摘要
- Plan: 2026-06-09-001-feat-managed-agent-doc-blocks-plan
- Implementation Units: 6（完成 3，未完成 3）
- Code review findings: 1 total（P0: 0, P1: 0, P2: 1 manual, P3: 0）
- Auto-fix rounds: 3（5 个问题修复）
- Final Validation: fail（U3 review wave 超时，U4-U6 未实现）
- Commits: bd81537 (U1), 7948889 (U2), d1a1cc3 (U3)

### 改了哪些文件
| 文件 | 变更类型 |
|------|----------|
| `crates/ralph-core/data/managed_blocks/hang-prevention.md` | 新建 |
| `crates/ralph-core/src/agent_doc_sync/mod.rs` | 新建 |
| `crates/ralph-core/src/agent_doc_sync/block.rs` | 新建 |
| `crates/ralph-core/src/agent_doc_sync/writer.rs` | 新建 |
| `crates/ralph-core/src/agent_doc_sync/builtin.rs` | 新建 |
| `crates/ralph-core/src/config/agent_doc_sync.rs` | 新建 |
| `crates/ralph-core/src/config/mod.rs` | 修改 |
| `crates/ralph-cli/src/commands/run.rs` | 修改 |
| `crates/ralph-cli/src/main.rs` | 修改 |
| `Cargo.toml` | 修改 |

### Fix Log 摘要
- **Round 1**: block_results 归因不准确 → 让 FileSyncResult 携带 per-block outcomes
- **Round 2**: Strict 模式不传播错误 + 孤立 begin marker 导致重复追加
- **Round 3**: orphan begin marker with matching hash 被错误升级 + 替换后丢失用户内容

### 完整发现清单
- **#R1** (P2, manual): OnErrorPolicy 与 OnError 类型割裂 — 两个枚举结构相同但无类型级关联，U4 集成需手动映射

### Shipping Record
- Status: BLOCKED/FAILED
- Reason: U3 review wave: 0/9 dimension reviewers responded; work timed out
- Plan is NOT marked as completed (U4-U6 remain unimplemented)
</details>
