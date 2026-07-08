---
title: "fix: ce-executor-serial handoff envelope P0/P1 Review 问题闭环"
type: fix
status: planned
date: 2026-07-07
created: 2026-07-07
execution_model: strictly-sequential-atomic-tdd
source_plan: docs/plans/2026-07-06-004-feat-ce-executor-serial-handoff-envelope-plan.md
origin: docs/brainstorms/2026-07-06-ce-executor-serial-handoff-envelope-requirements.md
scope: builtin:ce-executor-serial
review_source: "manual code review on 2026-07-07"
---

# fix: ce-executor-serial handoff envelope P0/P1 Review 问题闭环

## 背景

`docs/plans/2026-07-06-004-feat-ce-executor-serial-handoff-envelope-plan.md` 已实现 Handoff Envelope 的配置、payload validator、prompt 注入、`EmitResult` 摘要和 serial preset 接入，但本次 Review 发现仍有 1 个 P0 和 4 个 P1 风险：

1. **P0**：`presets/en/ce-executor-serial.yml` 仍有 copy-paste `ralph emit` 示例和 payload checklist 缺少 `handoff_envelope`，会诱导 hat 发出会被 validator 拒收的事件。
2. **P1**：`receiver_contract.to_hat` 的 registry-aware 校验只在纯 validator 中存在，真实 runtime / CLI policy-check pipeline 没有注入 `HatRegistry`，未知 `to_hat` 会被跳过。
3. **P1**：prompt renderer 对 `plan.completed_steps` 未做 escape，存在 prompt 注入面。
4. **P1**：`presets/schemas/ce-executor-serial.yml` 与 Rust validator 的 envelope 覆盖范围不一致，部分 payload-bearing topic 未要求顶层 `handoff_envelope`。
5. **P1**：缺少覆盖上述回归的集成测试、preset 示例扫描测试和文档 drift 验收。

本计划只修复 Review 中的 **P0/P1**。P2 级命名、轻微重复和注释优化不纳入本轮，避免扩大变更面。

## 目标

让 `builtin:ce-executor-serial` 的 handoff envelope 契约在 **preset instructions、schema SSOT、validator、runtime pipeline、CLI policy-check、prompt renderer、测试和 agent 文档** 中保持一致：

1. 任何 serial hat instructions 中的 emit 示例和 payload checklist 都不会再遗漏 `handoff_envelope`。
2. runtime 和 CLI `--policy-check` 使用同一套 registry-aware pipeline，未知 `receiver_contract.to_hat` 必须被拒收。
3. prompt 注入中所有 agent-controlled string 都经过 escaping，包括 `plan.completed_steps`。
4. serial schema 对所有需要 envelope 的 payload-bearing topic 明确要求顶层 `handoff_envelope`。
5. 新增测试能在未来 copy-paste 示例、schema、pipeline 或 escaping 退化时失败。

## 非目标

1. 不重新设计 `handoff-envelope.v1` 的 JSON schema。
2. 不迁移 `ce-executor-supervisor`、`ce-executor-pipeline` 或其它 preset。
3. 不让 base runtime 解析业务 markdown；继续遵守 `docs/solutions/logic-errors/base-runtime-must-not-parse-business-markdown.md`。
4. 不新增 CLI 参数或改变 `ralph emit` 的单业务事件预算。
5. 不把嵌套 envelope 字段塞进现有顶层 schema 机制；schema 只管顶层 `handoff_envelope`，嵌套结构仍由 Rust validator 校验。

## 代码事实

1. `crates/ralph-core/src/handoff_envelope.rs` 已有 payload validator、prompt view renderer 和 `unknown_to_hat` 纯 validator 测试。
2. `crates/ralph-core/src/validation/pipeline.rs` 已支持 `with_handoff_registry`，但调用方必须显式传入 registry。
3. `crates/ralph-core/src/event_loop/policy.rs` 的 `build_unified_validation_pipeline` 当前从 `ProtocolView` 构建 pipeline，没有向 validation context 注入 `HatRegistry`。
4. `crates/ralph-cli/src/policy_check.rs` 的 `run_policy_check_unified` 当前也从 `ProtocolView` 构建 pipeline，CLI dry-run 与 runtime 有同类缺口。
5. `presets/schemas/ce-executor-serial.yml` 是 serial event schema 的 SSOT；修改 `presets/en/ce-executor-serial.yml` 后必须检查并同步 schema。
6. `docs/brainstorms/2026-07-06-ce-executor-serial-handoff-envelope-requirements.md` 的 R15 首批迁移 topic 包括 `work.ready`、`work.done`、`work.failed`、`test.passed`、`test.failed`、`review.start`、`review.dimension.ready`、`review.dimension.done`、`review.dimensions.complete`、`review.complete`、`fix.applied`、`fix.exhausted`、`plan.complete`、`plan.blocked`、`REVIEW_COMPLETE`、`report.done`。

## 执行模型（强制）

```
U1 ──闭环──> U2 ──闭环──> U3 ──闭环──> U4 ──闭环──> U5
       ↑ 每个 Unit 必须 RED → GREEN → REFACTOR → Verify 通过才允许进入下一 Unit
```

| 规则 | 含义 |
|------|------|
| 严格串行 | 同一时间只做一个 Unit；Unit N 验收通过前禁止修改 Unit N+1 的实现文件 |
| 原子 TDD | 每个 Unit 先写会失败的测试，再做最小实现，最后只重构本 Unit 范围 |
| 低耦合 | Unit 测试只断言本 Unit 的输入输出，只有 U5 做跨链路回归 |
| 不扩大范围 | 不处理 P2，不迁移其它 preset，不顺手重构 validation pipeline |
| 文档同步 | preset、schema、agent skill guide 和 preset operator skill 必须在对应 Unit 同步 |

## 单元总览

| Unit | 交付物 | 覆盖问题 | 主要文件 |
|------|--------|----------|----------|
| U1 | runtime / CLI pipeline 注入 `HatRegistry` | P1 registry-aware 校验缺口 | `crates/ralph-core/src/event_loop/policy.rs`、`crates/ralph-cli/src/policy_check.rs`、`crates/ralph-core/src/validation/pipeline.rs` |
| U2 | prompt renderer escape `completed_steps` | P1 prompt 注入 | `crates/ralph-core/src/handoff_envelope.rs` |
| U3 | serial schema 与 Rust validator 覆盖范围收敛 | P1 schema drift | `presets/schemas/ce-executor-serial.yml`、`presets/en/ce-executor-serial.yml`、`crates/ralph-core/tests/scenarios/*.yml` |
| U4 | 修复 preset instructions / 示例 / agent 文档 | P0 instructions 诱导无效 emit | `presets/en/ce-executor-serial.yml`、`crates/ralph-core/data/*.md`、`skills/ralph-preset-common/references/*.md` |
| U5 | 跨链路回归、drift 检查和全量验证 | P0/P1 防复发 | tests、preset_lint、doc drift、full baseline |

## Unit 1: runtime / CLI validation pipeline 注入 `HatRegistry`

### 目的

让 `receiver_contract.to_hat` 的 unknown-hat 校验不只存在于纯 validator 测试中，而是进入真实 runtime 和 CLI policy-check 路径。

### 范围

修改：

1. `crates/ralph-core/src/event_loop/policy.rs`
2. `crates/ralph-cli/src/policy_check.rs`
3. 必要时补充 `crates/ralph-core/src/validation/pipeline.rs` 的构造 helper
4. 对应测试文件

不修改：

1. `HandoffEnvelopePayload` JSON 形态。
2. `HatRegistry` 的发现语义。
3. 非 config 场景下的 ad-hoc policy-check 行为。

### Red

先新增两个 pipeline 测试作为 Red：

1. CLI policy-check 对 unknown `to_hat` 当前应错误通过。
2. runtime unified pipeline 对 unknown `to_hat` 当前应错误通过。

### Green

实现方式：

1. 在 runtime pipeline 构建处，从 `RalphConfig` 的 hats / preset runtime config 构建 `Arc<HatRegistry>`。
2. `build_unified_validation_pipeline` 使用 `ValidationPipeline::from_registry` 或 `with_handoff_registry`，确保 validation context 中 `ctx.handoff_registry()` 为 `Some`。
3. `run_policy_check_unified` 在传入 config 的路径同样注入 `HatRegistry`。
4. 无 config 的 CLI dry-run 路径保留 registry 为 `None`，只能做 schema / topology 可得的校验，不得 panic。
5. 保持 topology preview 仍来自 `ProtocolView::from_event_loop_with_feature_hats`，不把 registry 注入替代 topology 校验。

### Refactor

1. 若 runtime 和 CLI 都需要同样的构造逻辑，提取一个窄 helper，返回 `Option<Arc<HatRegistry>>`。
2. helper 只做 registry 构建，不顺手改变 policy-check 的输出格式。

### Verify

1. unknown `to_hat` 在 CLI policy-check 中被拒收。
2. unknown `to_hat` 在 runtime unified pipeline 中被拒收。
3. known `to_hat` 仍可通过。
4. 无 config 的 policy-check 不 panic，错误信息仍可解释。

建议验收命令：

```bash
cargo nextest run -p ralph-cli --bin ralph -- policy_check_rejects_unknown_handoff_to_hat_from_builtin_serial_config
cargo nextest run -p ralph-core -- runtime_validation_pipeline_rejects_unknown_handoff_to_hat
```

## Unit 2: `completed_steps` prompt escaping

### 目的

关闭 prompt renderer 中剩余的 agent-controlled string 注入面，确保 `plan.completed_steps` 与 `root_goal`、`must_do`、`must_not_do` 等字段有同等级 escaping。

### 范围

修改：

1. `crates/ralph-core/src/handoff_envelope.rs`
2. 同文件测试

不修改：

1. Handoff Envelope JSON schema。
2. prompt section 的标题和字段顺序，除非测试证明现有输出无法安全表达。

### Red

先新增 `completed_steps_are_escaped_in_prompt_view` 作为 Red。重点覆盖：

1. 换行不能形成新的 markdown heading。
2. 反引号不能破坏 prompt code/span 结构。
3. 控制字符不能原样落入 prompt。
4. list truncation 后仍保持 escaping。

### Green

实现方式：

1. 将当前 `render_truncated_list` 改为对每个 item 调用 `escape_for_prompt`，或新增 `render_truncated_escaped_list` 并只用于 agent-controlled list。
2. 检查同文件所有 list 渲染调用点；只要来源可能来自 agent payload 或 plan/task 文本，都必须使用 escaped list。
3. 保持最多 5 项的 truncation 行为不变。

### Refactor

1. 用 helper 名称表达安全语义，例如 `render_truncated_escaped_list`。
2. 保留未 escaped helper 只用于纯静态枚举值；若没有明确用途，删除未 escaped helper。

### Verify

```bash
cargo nextest run -p ralph-core -- completed_steps_are_escaped_in_prompt_view
cargo nextest run -p ralph-core -- handoff_envelope
```

## Unit 3: serial schema 与 validator 覆盖范围收敛

### 目的

解决 `presets/schemas/ce-executor-serial.yml` 与 Rust validator 对 envelope-required topic 的不一致，避免 schema / instructions / runtime 三方漂移。

### 范围

修改：

1. `presets/schemas/ce-executor-serial.yml`
2. `presets/en/ce-executor-serial.yml` 中与 schema inline override 或 topic contract 相关的段落
3. `crates/ralph-core/tests/scenarios/ce_executor_serial_handoff_envelope_happy_path.yml`
4. 必要时更新 `crates/ralph-core/tests/scenarios.rs`

不修改：

1. 嵌套 envelope 字段的 Rust validator。
2. 非 serial preset schema。

### Red

先新增 `ce_executor_serial_schema_requires_handoff_envelope_for_all_migrated_topics` 作为 Red。

### Green

实现方式：

1. 建立本轮权威 topic 集合：
   - `work.ready`
   - `work.done`
   - `work.failed`
   - `test.passed`
   - `test.failed`
   - `review.start`
   - `review.dimension.ready`
   - `review.dimension.done`
   - `review.dimensions.complete`
   - `review.complete`
   - `fix.applied`
   - `fix.exhausted`
   - `plan.complete`
   - `plan.blocked`
   - `REVIEW_COMPLETE`
   - `report.done`
2. 对上述 topic 中会携带 payload 的 schema entry，`required_fields` 必须包含顶层 `handoff_envelope`。
3. 若某个 topic 在实际 runtime 中是 terminal alias、无 payload 或不在 schema 中管理，必须在测试 fixture 里显式列入 allowlist，并在 schema 附近注释原因。
4. 更新 happy-path BDD 场景，让新增 envelope-required topic 的 mock payload 也包含合法 `handoff_envelope`。
5. 保持 `task.resume` 例外不变；该例外属于 runtime correction，不是业务 handoff。

### Refactor

1. 不复制嵌套 validator 规则到 YAML。
2. 用 YAML anchor 或局部注释降低重复，但不得让 schema 可读性下降。

### Verify

```bash
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
cargo nextest run -p ralph-core -- ce_executor_serial_handoff_envelope_happy_path
```

## Unit 4: 修复 serial preset instructions、示例和 agent 文档

### 目的

修复 P0：hat instructions 中不能继续存在会诱导 agent 发出无效 payload 的旧示例或旧 checklist。

### 范围

修改：

1. `presets/en/ce-executor-serial.yml`
2. `crates/ralph-core/data/ralph-tools-emit.md`
3. 必要时更新 `crates/ralph-core/data/ralph-tools.md`、`crates/ralph-core/data/ralph-tools-cmdref.md`
4. 若 preset author / review skill 依赖 envelope contract，更新 `skills/ralph-preset-common/references/agent-native-model.md`、`author-checklist.md`、`patterns.md`、`finding-rubric.md`

不修改：

1. Rust validator。
2. schema 规则；schema 已在 U3 处理。
3. `CLAUDE.md` / `AGENTS.md`，除非本轮改变 builtin preset 列表或硬规则。

### Red

先新增 `ce_executor_serial_emit_examples_include_handoff_envelope` 作为 Red。重点覆盖已知旧段落：

1. `work.done` emit 示例。
2. `test.passed` / `test.failed` emit 示例。
3. `review.dimension.ready` emit 示例。
4. `plan.complete` payload checklist。
5. `fix.applied` / `fix.exhausted` payload instructions。

### Green

实现方式：

1. 将每个 envelope-required topic 的 payload checklist 明确列出顶层 `handoff_envelope`。
2. 所有 copy-paste `ralph emit` 示例必须包含合法最小 envelope，或改成引用 `ralph-tools-emit` 的 envelope section 并在示例中保留 `handoff_envelope: { ... }` 占位。
3. 删除或改写“只需要 N 个字段”这类与 envelope-required contract 冲突的文案。
4. Hat instructions 涉及 `ralph emit` 时必须继续要求先跑 `--policy-check`，符合项目 hard rule。
5. `crates/ralph-core/data/ralph-tools-emit.md` 增加一段 serial handoff envelope 使用说明：说明 `builtin:ce-executor-serial` 中 envelope-required topic 必须携带顶层 `handoff_envelope`，并提示嵌套字段由 Rust validator 校验。
6. 检查 preset operator skill 文档是否需要新增 AAF review checklist：preset reviewer 应识别 envelope-enabled preset 中 instructions / schema / validator 不一致的问题。

### Refactor

1. 避免在每个 hat instructions 大段复制同一份 envelope schema；优先引用 `ralph-tools-emit` 并保留最小有效示例。
2. 示例中的 `task_id`、`task_key`、`step` 必须继续强调从 `ralph tools task list` 获取，不得手写闭合任务 id。

### Verify

```bash
cargo nextest run -p ralph-cli --bin ralph -- ce_executor_serial_emit_examples_include_handoff_envelope
scripts/check-cli-doc-drift.sh
```

如果修改了 `crates/ralph-core/data/*.md` 中带源码行号的引用，必须用 `sed -n 'NN,MMp' <file>` 逐项复核引用范围仍指向正确代码。

## Unit 5: 跨链路回归、drift 检查和全量验收

### 目的

把 U1-U4 的局部修复串成一次完整回归，确认 Review 的 P0/P1 不是只在单元测试里被修掉。

### 范围

只补必要集成测试、BDD 场景和验收脚本调用；不再引入新生产行为。

### Red

如果 U1-U4 已全绿，本 Unit 的 Red 主要来自新增跨链路场景：

1. `ce_executor_serial_handoff_envelope_unknown_to_hat_is_rejected`
   - 真实 EventLoop runner 或最接近 runtime 的 validation path。
   - payload 使用合法 envelope，但 `receiver_contract.to_hat = "ghost-hat"`。
   - 断言事件被拒收，且不会继续推进到下一个 hat。

2. `ce_executor_serial_handoff_envelope_terminal_topics_accept_valid_envelope`
   - 覆盖 `plan.complete`、`plan.blocked`、`REVIEW_COMPLETE`、`report.done` 中实际携带 payload 的 topic。
   - 断言有效 envelope 不被 schema 或 Rust validator 误拒。

### Green

实现方式：

1. 在 `crates/ralph-core/tests/scenarios/` 下补充最小 BDD fixture，必须走真实 runtime path；禁止只做 source-only placeholder。
2. 将 U3 的 schema 测试和 U4 的 preset 示例扫描测试保留为长期防漂移护栏。
3. 跑 preset schema 校验、SSOT byte-equality、doc drift 和全量测试。

### Refactor

1. 去除为调试新增的临时 fixture 或日志。
2. 确认没有把 Review 临时说明写进用户-facing prompt。

### Verify

最终验收必须通过：

```bash
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
cargo nextest run -p ralph-core -- ce_executor_serial_handoff_envelope
scripts/check-cli-doc-drift.sh
./scripts/run-tests.sh
```

## 验收标准

本计划完成后，必须同时满足：

1. P0 stale instructions 修复：`presets/en/ce-executor-serial.yml` 中 envelope-required topic 的示例 / checklist 不再遗漏 `handoff_envelope`。
2. P1 registry-aware pipeline 修复：CLI policy-check 和 runtime validation pipeline 都拒收未知 `receiver_contract.to_hat`。
3. P1 prompt escaping 修复：`completed_steps` 中的换行、反引号、控制字符不会原样进入 prompt。
4. P1 schema drift 修复：serial schema 对所有权威 topic 的 payload contract 与 Rust validator 一致；例外必须有测试 allowlist 和注释。
5. P1 测试覆盖补齐：核心路径、异常路径、schema/preset 示例 drift、terminal topic 都有测试。
6. 文档同步：agent skill guide 和 preset operator skill 不再描述旧 contract；`scripts/check-cli-doc-drift.sh` 通过。
7. 安全合规：不新增 hardcoded token，不把 envelope payload 中的敏感字段扩散到日志；新增错误信息只包含 topic、hat id、field path 和 reason，不打印完整 payload。

## 风险与缓解

| 风险 | 缓解 |
|------|------|
| schema 要求 envelope 的 topic 过宽，误伤 terminal event | U3 先建立权威 topic 集合；真实无 payload topic 必须显式 allowlist 并注释 |
| registry 构造改变无 config policy-check 行为 | U1 保持 no-config path registry 为 `None`，只在真实 config path 增强校验 |
| preset 示例扫描测试过脆 | 限定扫描 envelope-required topic 段落和 fenced code block，避免全文件裸正则 |
| instructions 大量复制 envelope schema 后再次漂移 | U4 只保留最小有效示例，详细规则集中到 `ralph-tools-emit.md` |
| prompt escaping 改变可读性 | U2 只 escape agent-controlled string，保留标题、字段名和 truncation 格式 |

## 需要人工确认的点

1. `LOOP_COMPLETE` 是否属于本轮 envelope-required topic：
   - 原始 R15 未列入 `LOOP_COMPLETE`，但现有 reporter 可能携带 terminal payload。
   - 若实际 schema / runtime 将其视为无 payload sentinel，则不纳入 U3 权威集合。
   - 若 reporter instructions 要求携带 JSON payload，则必须纳入 schema、instructions 和测试。

2. `REVIEW_COMPLETE` 与 `review.complete` 是否都仍是活跃 topic：
   - 若二者之一只是 legacy alias，应在 U3 测试 allowlist / 注释中写清楚。
   - 不允许 schema silently 漏掉仍会被 hat emit 的活跃 topic。
