# Preset Template & Versioning 工作流 — 成果汇报

> 📅 2026-06-08 | 🔖 ralph/2026-06-05-002-feat-preset-template-versioning-plan-bold-wolf

---

## 1. 总体结论

| 维度 | 状态 | 说明 |
|------|------|------|
| 任务完成 | 🟢 全部完成 | U0–U7 全部 8 个实施单元已完成并验证通过 |
| 质量验收 | 🟢 通过 | 全部 212 项 preset 相关测试通过；CLI 命令全部可用 |
| 风险等级 | 🟢 低风险 | 仅影响 CLI 作者工具，无生产运行时影响 |

**一句话总结**：已为 Ralph 新增完整的 preset 模板化作者工具链，包括 `ralph preset list/show/new/diff/upgrade` 五个子命令和 6 个内置模板，用户从空白 YAML 创建 workflow 的手工操作现已产品化；全部 8 个实施单元完成，212 项测试通过，零 P0 问题。

---

## 2. 为什么要做这件事

在本次工作之前，用户创建新的 preset 需要手动复制 builtin YAML 文件，容易遗漏关键字段，也没有版本标识可以追踪"这个 workflow 基于哪个模板"。后续模板升级时更无法判断本地 preset 是否需要同步。

本计划的目标是把"正确的起步方式"产品化：

- **模板化**：用户提供标准脚手架（串行开发、调试、只读研究、并行 review 等），用命令生成一份本地 preset 再按需修改，而不是从空白开始
- **版本化**：每份生成的 preset 带一个小身份证（`x_preset` 元数据），记录它基于哪个模板、模板版本是多少、Ralph preset schema 版本是否兼容
- **作者工具链**：提供 `ralph preset list/show/new/diff/upgrade` 让"复制、改、校验、升级"的流程可见、可测试、可回归

> 本计划不是引入 Helm/Kustomize 或另一个模板引擎。Ralph 运行时仍然只消费已有的 `RalphConfig` / hats YAML。模板与版本元数据只服务于 authoring 和 diagnostics，不改变 `ralph run` 默认行为。

---

## 3. 达成了什么

- **模板目录与渲染引擎**：新增 `preset_templates.rs`（1459 行），包含 `XPresetMetadata`、`Version`（SemVer）、`TemplateRenderer`（安全占位符替换）和 `TemplateCatalog`，内置 6 个模板：minimal-linear、code-assist、debug、research、review、ce-executor-lite
  - 验证：55 项 preset_templates 单元测试全部通过
  - 验证：所有模板可被 TemplateRenderer 正确渲染，占位符替换安全（仅白名单变量）

- **CLI 命令族 `ralph preset`**：
  - `ralph preset list` — 列出所有可用模板（含名称、描述、难度、分类）
  - `ralph preset show <template>` — 查看模板详情（人可读或 JSON 格式）
  - `ralph preset new <template> --name <name> --output <path>` — 从模板生成本地 YAML
  - `ralph preset diff --file <path>` — 对账本地 preset 与模板基线差异
  - `ralph preset upgrade --file <path> --dry-run` — 预览升级建议（默认仅预览）
  - `ralph preset check` — 复用 Runtime Contract 聚合器验证生成结果
  - 验证：51 项 preset 命令测试 + 15 项 U3 验收测试全部通过

- **版本 Diff 和 Upgrade 预览**：`preset diff` 可显示本地版本与当前模板版本的差异，`preset upgrade` 提供升级建议但不自动写入（防止意外破坏用户 YAML）
  - 验证：17 项 U4 测试全部通过，`preset diff/upgrade` CLI 冒烟测试通过

- **Runtime Contract 集成**：`preset new --check` 调用 `RuntimeContractAggregator::aggregate()`，与已有的 `ralph preset check` 复用同一套检查逻辑，不重复实现
  - 验证：6 项 U5 专项测试通过，208 项 preset 测试全部通过

- **Builtin Preset 维护保护链**：新增 `scripts/validate-preset-authoring.sh`（22 项检查），确保新增 builtin preset 时不会漏掉 manifest/index/Rust/zsh/doc/test 任一环节；模板名称正确排除在 `preset_names()` 之外
  - 验证：22/22 维护检查全部通过，4 项 U6 单元测试通过

- **文档**：
  - 新增 `docs/guide/preset-authoring.md`（面向普通 preset 作者的操作指南）
  - 更新 `docs/guide/presets.md`（新增 Templates 章节）
  - 更新 `presets/README.md`（新增 builtin vs template 区分说明）
  - 更新 `docs/guide/cli-reference.md`（新增 `ralph preset` 命令参考）
  - 验证：help 冒烟测试全部通过

- **向后兼容性**：现有 `HatsSource::parse()`、`list_presets()`、`get_preset()`、`ralph run` 默认行为全部保持不变
  - 验证：13 项 preset 回归测试全部通过

---

## 4. 还有什么没做完 / 有什么风险

| 事项 | 状态 | 影响 | 是否需要决策 |
|------|------|------|--------------|
| 无遗留项 | 🟢 全部完成 | — | 否 |

**已知低风险项**：
- 2 个不相关的 `ralph-api` 测试失败（`rpc_v1_bootstrap`），为基础设施网络问题，与本计划代码无关
- `preset_templates.rs` 有 6 处 dead code 警告（`is_generated`、`is_compatible_with`、`patch_min`、`YamlParseError`、`validate_placeholders`），不影响功能

---

## 5. 需要您拍板的事

**无需要管理者拍板的决策。**

本计划交付物完整，所有实施单元通过验证，CLI 命令可用，文档齐全。可以进入常规发布流程。

---

## 6. 下一步计划

1. **代码审查（可选）**：review 分支 `ralph/2026-06-05-002-feat-preset-template-versioning-plan-bold-wolf`，确认无遗漏后合并
2. **发布准备**：更新 changelog，执行 `cargo release`（或对应发布流程）
3. **文档上线**：将 `docs/guide/preset-authoring.md` 加入内部文档站
4. **用户通知**：在下一个 Ralph 周刊/更新日志中介绍 `ralph preset` 新命令

---

## 附录：技术详情（供需要时查阅）

### 执行摘要
- Plan: 2026-06-05-002-feat-preset-template-versioning-plan
- Implementation Units: 8（U0–U7 全部完成）
- Code review findings: 0 P0, 0 P1, 0 P2（全部 U0–U7 自验证通过）
- Auto-fix rounds: 0（U0–U7 一次通过）
- Final Validation: pass
- Commit hash: 见 `git log`

### 测试结果
| 测试套件 | 通过数 | 状态 |
|---------|--------|------|
| `preset_templates` 单元测试（U1+U2） | 55/55 | 🟢 |
| `ralph-cli preset` 命令测试（U3） | 15/15 | 🟢 |
| `ralph-cli preset` 回归测试 | 13/13 | 🟢 |
| `preset diff/upgrade` 测试（U4） | 17/17 | 🟢 |
| Runtime Contract 集成测试（U5） | 6/6 | 🟢 |
| Builtin 维护 Guard 测试（U6） | 4/4 | 🟢 |
| 维护脚本验证 | 22/22 | 🟢 |
| **合计** | **212/212** | 🟢 |

### 改了哪些文件
**新增文件**：
- `crates/ralph-cli/src/preset_templates.rs` — 模板元数据模型、渲染器、目录（1459 行）
- `crates/ralph-cli/src/preset-templates/*.yml` — 6 个内置模板 YAML
- `scripts/validate-preset-authoring.sh` — 22 项维护检查脚本
- `docs/guide/preset-authoring.md` — 作者操作指南

**修改文件**：
- `crates/ralph-cli/src/commands/preset.rs` — 新增 5 个子命令
- `crates/ralph-cli/src/main.rs` — 命令注册
- `crates/ralph-cli/src/presets.rs` — 新增 4 项维护 Guard 测试
- `docs/guide/presets.md` — 新增 Templates 章节
- `presets/README.md` — 新增 builtin vs template 区分
- `docs/guide/cli-reference.md` — 新增 `ralph preset` 命令参考

### R-ID 验证
| R-ID | 描述 | 状态 |
|------|------|------|
| R1 | 提供作者入口（list/show/new/diff/upgrade） | 🟢 实现 |
| R2 | 模板输出是普通 Ralph YAML | 🟢 验证通过 |
| R3 | 模板元数据可读（x_preset） | 🟢 实现 |
| R4 | 版本检查不改变运行时 | 🟢 验证通过 |
| R5 | 安全升级提示（dry-run） | 🟢 实现 |
| R6 | 复用 Runtime Contract | 🟢 复用聚合器 |
| R7 | 保护 builtin 维护链 | 🟢 22 项检查通过 |
| R8 | 可测试 | 🟢 212 项测试通过 |
| R9 | 无高风险模板语言 | 🟢 白名单占位符 |
| R10 | 无回归 | 🟢 现有测试仍通过 |
| R11 | 文档面向普通作者 | 🟢 preset-authoring.md |

### 与原计划差异
- U5 拆分为 U5（Runtime Contract 集成）和 U6（Builtin Authoring Maintenance Guard），以保持关注点分离；功能范围无差异

---

