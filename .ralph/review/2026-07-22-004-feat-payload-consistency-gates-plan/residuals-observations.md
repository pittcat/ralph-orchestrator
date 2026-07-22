# Residuals observations

本记录收敛本计划评审中保留的观察项；这些项目不改变本次修复范围，也不阻塞本计划的文档与格式化验收。后续实现应单独建立 housekeeping 计划，重新评估影响面、验收标准与回滚边界。

## S4：rustfmt 漂移

- **事实**：U3/U5 引入或调整的 Rust 代码可能带来格式漂移，属于机械性质量问题而非运行时语义问题。
- **本次处置**：已执行 `cargo fmt --all`，随后以 `cargo fmt --all -- --check` 复核通过。
- **后续路由**：保留常规格式化门禁；不单独引入生产代码修复。

## S5：注入 skill 源码行号漂移

- **事实**：注入给 agent 的 skill 若引用源码行号，源码移动后可能导致定位失真。
- **本次处置**：U6 已复核并修正 `NULL_PAYLOAD_REJECT_TOPICS` 的行号锚点；本节与 U6 重叠，不再重复修改。
- **后续路由**：后续源码重排时同步复核 `crates/ralph-core/data/*.md` 中的源码锚点，避免把行号当作稳定 API。

## M1：双 preset rules 复制

- **事实**：两处 preset 中存在约 31 行重复的 payload consistency rules；当前 preset 定义采用内联内容，没有 preset 继承或共享片段机制。
- **影响**：规则修订需要人工同步多个位置，存在漏改和语义漂移风险。
- **建议**：housekeeping 计划评估引入受 lint 约束的 preset 继承或共享规则来源，并补充结构化 parity 验证；本次不改 preset YAML。

## C1：fail-close 设计观察

- **事实**：`WHITELISTED_PREDICATE_OPS` 当前固定支持 6 个操作符，非法操作与部分类型不匹配路径采用 fail-close；现有测试覆盖主要拒收路径，但类型系统尚未由 EventSchema 字段类型统一约束。
- **影响**：新增操作符或扩展字段类型时，需要同时维护 runtime evaluator、preset lint 与测试，否则可能出现规则作者预期和运行时判定不一致。
- **建议**：housekeeping 计划评估引入 EventSchema field types，并在扩展操作符时同步补齐 lint、运行时和 BDD 覆盖；本次不改变 gate 语义。

## M2：first_op 重构

- **事实**：当前 evaluator 对谓词操作符采用手工分支匹配，操作符集合扩展时存在遗漏分支或不可达分支的维护风险。
- **影响**：从 6 个操作符扩展到更多操作符时，runtime 与 lint 的支持列表可能短暂不一致。
- **建议**：下一次扩展操作符时一并评估以集中式 op table 替代手工匹配，并让白名单、解析和评估共享同一来源；本次不做结构重构。

## A5：rule.message 转义漏洞

- **事实**：`rule.message` 进入提示或诊断文本时，现有 `escape_for_prompt` 只覆盖有限字符，未覆盖反引号、换行、零宽字符或 ANSI 控制序列等输入。
- **影响**：恶意或异常规则消息可能破坏提示格式、降低可读性，甚至影响 agent 对诊断边界的判断；该函数还被其他 envelope 路径复用。
- **建议**：housekeeping 计划单独评估扩大转义集合并增加 lint/回归测试，且必须同时验证 `handoff_envelope.rs` 的调用方；本次不修改转义逻辑。

## G1：并行 plan commit 拓扑

- **事实**：评审记录的 13 个提交中有 5 个提交不属于本计划的 payload consistency 变更范围（包括 supervisor 相关提交与 concepts 文档提交）。
- **影响**：若直接按提交拓扑推断本计划边界，可能把无关改动误归因于本计划，影响 diff 审计和回滚判断。
- **建议**：housekeeping 计划在 baseline 收尾阶段统一处理提交归属与范围审计；本次保持现有历史，不重写提交、不提交新 commit。
