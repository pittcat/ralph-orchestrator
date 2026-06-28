## 审查概要
- **Commit/PR**: 当前工作区 diff（面向 `docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md` 与 `docs/plans/2026-06-27-002-feat-mechanism-foundation-completion-plan.md` 的机制底座接线变更）
- **审查范围**: `crates/ralph-core/src/event_loop/mod.rs`, `crates/ralph-core/src/event_loop/stages/emit_schema_gate_stage.rs`, `crates/ralph-core/tests/scenarios.rs`, `crates/ralph-proto/src/event_bus.rs`
- **总体结论**: REQUEST_CHANGES
- **风险等级**: HIGH

### P0 - 阻断问题
1. **[安全风险 / 隐藏副作用]** 生产路径裸 `eprintln!` 会把事件 payload 与调度状态写入 stderr，造成敏感数据泄漏与协议输出污染。
   - **位置**: `crates/ralph-core/src/event_loop/mod.rs:9434`
   - **详细分析**: `apply_emit_gate_on_validated` 在每个 validated event 上打印 `event.payload`，该 payload 可能包含用户 prompt、`human.guidance`、任务路径、诊断上下文、review findings 或后续 agent 输出；攻击者只要把敏感内容放入合法 event payload，就能让 Ralph 在 stderr 中永久暴露该内容。相同 diff 还在 isolated hot path 与 `EventBus::publish` 中增加多处 `DBG` 输出，调用者无法通过日志级别关闭。
   - **潜在影响**: CI / 终端日志 / agent transcript 中泄漏敏感计划内容或用户输入；stderr 被上层工具当成协议诊断流消费时，还会造成难以复现的交互污染。
   - **修复建议**: 删除所有 `eprintln!("DBG ...")`；确需诊断时改为 `tracing::debug!`，默认关闭，并只记录 topic / reason_code 等非 payload 字段；payload 必须显式脱敏或摘要化。
   - **验证方式**: `rg 'eprintln!\("DBG' crates/ralph-core crates/ralph-proto` 返回 0；补一个捕获 stderr 的单元测试，构造 payload 含 `SECRET_SENTINEL`，确认运行路径不会输出该字符串。

### P1 - 严重问题
1. **[兼容性破坏 / 隐式契约绕过]** `review.complete` 从默认 emit schema gate 删除，但默认 pipeline 没有注入 preset schema，`publish_event` 路径会放行缺字段的 `review.complete`。
   - **位置**: `crates/ralph-core/src/event_loop/stages/emit_schema_gate_stage.rs:37`
   - **详细分析**: 本 diff 删除了 `default_required_fields()` 中 `review.complete -> ["fix_plan_file", "verdict"]` 的 baseline gate，并在注释中要求 preset-specific contracts 通过 `EmitSchemaGateStage::new(preset_required_fields)` 注入；但 `StagePipeline::with_default_stages` 仍硬编码 `EmitSchemaGateStage::with_defaults()`，`build_stage_pipeline_from_config` 也只从 config 解析 flow，不读取 `presets/schemas/ce-executor-serial.yml` 的 `required_fields`。因此 `publish_event(Event::new("review.complete", "{}"))` 会通过 stage gate 并进入 bus，而 `ce-executor-serial` schema 明确要求 `fix_plan_file`, `verdict`, `residual_findings_count` 等字段。
   - **潜在影响**: coordinator 被 `review.complete` 激活后读取不到 `fix_plan_file` / `verdict`，复现机制底座原本要消灭的半完成或错误收口路径；JSONL ingest 可能仍被旧 event-policy 挡住，但 `publish_event` 这个公共调用面已经形成合同分叉。
   - **修复建议**: 让 `build_stage_pipeline_from_config` 从 `config.event_loop.schemas` / `ProtocolView` 构造 required-fields 表并传入 `EmitSchemaGateStage::new`；若无 schema 才回退 `with_defaults()`。同时补 `publish_event` 回归测试：`review.complete` 缺 `fix_plan_file` / `verdict` 必须 reject，完整 payload 才能进 bus。
   - **验证方式**: 新增测试覆盖 `publish_event(review.complete, "{}")` 不进 bus 且 recovery envelope reason 为 `missing_required_fields`；再覆盖 ce-executor schema 中所有 required_fields 被 stage gate 使用，而不是只由 legacy policy 使用。

2. **[性能风险 / 资源耗尽]** `EventBus::publish` 每次 publish 都克隆并格式化整个 pending 队列状态，事件风暴下会把 O(事件分发) 放大成 O(事件数 × hat 数 × pending 队列大小) 的 stderr 洪泛。
   - **位置**: `crates/ralph-proto/src/event_bus.rs:162`
   - **详细分析**: `self.pending.iter().map(... q.len()).collect::<Vec<_>>()` 在每次事件发布时都会分配 Vec、克隆 hat id、格式化 recipients 和 pending_sizes；mechanism foundation 正在增加 repair/recovery/diagnostic 事件，攻击者可以构造大量合法 topic，让调度本身被日志格式化拖垮。
   - **潜在影响**: 长 loop 或 wave 场景中 stderr 迅速膨胀，CI 日志超限、运行变慢，甚至掩盖真正的 rejection envelope。
   - **修复建议**: 删除该 `eprintln!`；如需观测，使用 `tracing::debug!(topic, recipient_count = recipients.len())`，不要打印完整 pending map。
   - **验证方式**: 静态检查无 `DBG bus.publish`；压力测试或单元测试可断言 publish 不写 stderr，性能不再随 pending map 格式化产生额外分配。

### P2 - 建议
1. **[测试不足 / 维护成本]** 测试诊断输出继续使用 `eprintln!`，会让 scenario 失败日志依赖非结构化文本。
   - **位置**: `crates/ralph-core/tests/scenarios.rs:568`
   - **详细分析**: 测试侧打印 parsed topics 与复杂 assert message 对排查有帮助，但它没有和 scenario artifact 结构绑定；后续失败时仍需要人工从 stderr 中拼上下文。
   - **潜在影响**: 不影响生产行为，但会增加 BDD 回归排查成本，并让大批 scenario 失败时日志噪声过高。
   - **修复建议**: 保留增强后的 assert message；把 per-iteration parsed topics 写入已有 snapshot / scenario diagnostic 结构，或只在断言失败分支输出。
   - **验证方式**: 失败时 artifact 中能看到 parsed topics、seen_topics、recorded_rejections；正常通过时不产生大量 stderr。

## 兼容性评估
- **API 变更**: `publish_event` 的实际语义被放宽，`review.complete` 缺字段可进入 bus；这与 `ce-executor-serial` 的 schema / coordinator prompt 契约不兼容。
- **数据格式变更**: 本 diff 未改持久化格式，但新增 stderr 输出会把 payload 内容复制到日志 / transcript。
- **依赖变更**: 无新增依赖。
- **回滚安全性**: 删除调试输出可安全回滚；恢复 `review.complete` required-fields baseline 或完成 schema 注入也可安全回滚，前提是补齐 publish/jsonl 双路径测试。

## 测试充分性评估
- **新增测试覆盖**: 当前 diff 主要增强 scenario 失败诊断，没有新增能证明 `review.complete` schema 注入存在的测试。
- **缺失测试场景**: 缺少 `publish_event(review.complete, "{}")` must-reject；缺少 stderr 不泄漏 payload 的回归；缺少 `StagePipeline::with_default_stages` 使用 config schema 而非默认表的断言。
- **回归风险**: `ce-executor-serial` review synthesis 到 coordinator 的链路最容易受影响；需要验证 malformed `review.complete` 不会激活 coordinator，也不会生成错误 `plan.complete`。

## 8项审查维度覆盖
- **隐藏副作用**: 命中，裸 `eprintln!` 改变生产可观测输出并泄漏 payload。
- **兼容性破坏**: 命中，`review.complete` runtime gate 与 schema SSOT 分叉。
- **边界情况**: 命中，缺字段 `{}` / payload 含敏感大文本 / 事件风暴会走未保护路径。
- **性能风险**: 命中，hot path 格式化 pending map 与多处 per-event stderr。
- **安全风险**: 命中，payload 可含敏感信息并被无条件输出。
- **命名误导**: 命中，注释声称 preset-specific contracts “MUST be injected”，但默认 pipeline 未注入。
- **测试不足**: 命中，缺少 publish 路径 schema contract 回归与 stderr 泄漏回归。
- **维护成本**: 命中，调试打印分散在核心调度、JSONL ingest、bus publish，后续很容易遗漏清理。

## 对抗性审查声明
> 本审查基于对抗性原则执行，已排查语义欺骗、隐藏副作用、边界漏洞和连锁反应风险。未运行测试，按用户要求仅做代码审查并写入 review 文档。
