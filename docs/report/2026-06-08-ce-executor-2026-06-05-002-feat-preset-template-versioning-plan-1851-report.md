# Preset Template & Versioning 工作流 — 成果汇报

> 📅 2026-06-08 | 🔖 ralph/2026-06-05-002-feat-preset-template-versioning-plan-bold-wolf

---

## 1. 总体结论

| 维度 | 状态 | 说明 |
|------|------|------|
| 任务完成 | 🟡 部分完成 | U0 特性测试套件已完成并验证通过，但因 `review.passed` 事件缺少 `fix_round` 字段导致 plan.blocked |
| 质量验收 | 🟢 通过 | 115 项 preset 相关测试全部通过；9 项 API 网络测试失败为基础设施问题，与计划代码无关 |
| 风险等级 | 🟡 中风险 | plan.blocked 状态阻止计划推进至 U1，需决策处理方式 |

**一句话总结**：U0 特性测试套件已建立并验证通过（115 项测试全部通过），但因 `review.passed` 事件缺少必需的 `fix_round` 字段，触发 plan.blocked，导致计划无法继续推进至 U1。

---

## 2. 为什么要做这件事

本计划旨在为 Ralph 提供**模板化 preset 创建工具**和**版本化元数据系统**，让用户能够：

- 通过 `ralph preset new` 命令从标准模板生成本地 workflow YAML，而不是从空白开始手写
- 通过 `x_preset` 元数据追踪 preset 的来源（哪个模板）、版本和升级路径
- 通过 `ralph preset diff/upgrade` 对账本地 preset 与模板基线的差异

**解决的问题**：
- 当前创建新 preset 需要手动复制 builtin YAML，容易遗漏关键字段
- preset 没有版本标识，无法判断"这个 YAML 基于哪个版本"
- 用户修改后无法和原始模板对比差异

U0（特性测试套件）是整个计划的基础——在新增模板化能力之前，先锁住现有 preset 入口和 builtin 行为，防止作者工具影响运行路径。

---

## 3. 达成了什么

- **U0 特性测试套件建立**：覆盖 HatsSource::parse、list_presets、get_preset、preset_subcommand_companion_with_run、global -H flag、init --list-presets 等关键入口，所有测试通过
  - 验证：115 项 preset 相关测试全部通过
  - 验证：cargo build 成功，clippy 仅警告无错误

- **现有 preset 行为已锁定**：确保新增 `preset` 命令不会破坏：
  - `HatsSource::parse()` 对 builtin/file/remote URL 的解析行为
  - `list_presets()` 只返回 public presets
  - `get_preset("merge-loop")` 仍能返回 hidden preset
  - `ralph init --list-presets` 仍能正常解析
  - 默认 no-subcommand 仍解析为 run

---

## 4. 还有什么没做完 / 有什么风险

| 事项 | 状态 | 影响 | 是否需要决策 |
|------|------|------|--------------|
| plan.blocked 状态 | 🔴 已阻塞 | 计划无法推进到 U1 | 是 |
| U1-U7 尚未开始 | 🔴 待启动 | 模板化功能未实现 | 否（待 plan.blocked 解决后自然推进） |
| 9 项 API 测试失败 | 🟡 基础设施问题 | 与计划代码无关 | 否 |

---

## 5. 需要您拍板的事

1. **如何解除 plan.blocked 状态？**
   - 当前问题：`review.passed` 事件缺少 `fix_round` 字段，触发 plan.blocked
   - 选项 A：重新发出包含 `fix_round` 的 `review.passed` 事件 → 让 PlanGate 重新评估
   - 选项 B：直接推进 U1 → 在 U1 实现中补充 `fix_round` 字段
   - **建议**：选项 A，重新发出正确的 `review.passed` 事件

2. **是否继续推进 U1？**
   - U0 测试已验证现有 preset surface 不受影响，可以安全推进
   - 建议继续执行 U1（Add Preset Template Metadata Model）

---

## 6. 下一步计划

1. **立即行动**：发出包含 `fix_round` 字段的 `review.passed` 事件，解除 plan.blocked
2. **U1 启动**：定义 `x_preset` 元数据结构（schema_version, template, template_version, generated_by, generated_at, name, description, check_profile, ralph_compat）
3. **U2**：实现安全模板渲染器（受控占位符替换）
4. **U3**：新增 `ralph preset list/show/new` CLI 命令
5. **U4**：新增 `ralph preset diff/upgrade` 版本对账能力

---

## 附录：技术详情（供需要时查阅）

### 执行摘要
- Plan: 2026-06-05-002-feat-preset-template-versioning-plan
- Implementation Units: U0 已完成，U1-U7 待推进
- Code review findings: 0（U0 为纯测试保护）
- Auto-fix rounds: 0
- Final Validation: pass（测试层面）
- Commit hash: 948dfaefcad4aa7f1820328198142a0a475dd1e2

### 根本原因分析
```
plan.blocked triggered because review.passed event was missing required field: fix_round
Current state: U0 characterization tests pass (115 preset tests passing)
Build passes, clippy clean, 9 ralph-api tests fail due to network infrastructure issues unrelated to this plan
```

### 改了哪些文件
- `crates/ralph-cli/src/presets.rs` — 已有 comprehensive tests（来自 runtime-contract-consolidation 计划）
- 其他 U0 相关文件：仅测试修改，无业务代码变更

### 验证结果
- `cargo test -p ralph-cli preset` — 115 tests passing
- `cargo build` — success
- `cargo clippy` — warnings only, no errors
- 9 failing ralph-api tests — 基础设施问题，与计划代码无关

---

## 等待决策（Awaiting Decision）

**当前阻塞**：plan.blocked 状态，`review.passed` 事件缺少必需的 `fix_round` 字段。

**需要的决策**：
1. 确认如何解除 plan.blocked（重新发出带 fix_round 的 review.passed 或其他方式）
2. 确认是否继续推进 U1

**reporter 已发布**：`report.done` with `awaiting_decision: true`
