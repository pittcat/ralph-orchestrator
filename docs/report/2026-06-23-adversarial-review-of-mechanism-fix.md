---
title: 对抗性代码审查报告：ce-executor-serial 机制层修复（typed RejectionKind）
date: 2026-06-23
type: adversarial-review
reviewer: claude-m3 (adversarial-worker)
target_branch: pittcat-dev (working tree, uncommitted)
target_diff:
  - crates/ralph-core/src/hat_handoff/gate.rs (+188 行)
  - crates/ralph-core/src/preset/engine/gates.rs (+77 行)
  - task.md (-71 行，疑似误改工作区根文件)
context: docs/report/2026-06-23-ralph-e2e-ce-executor-serial-loop-20260622-182705-diagnosis.md
fix_plan_doc: 未生成（缺失）
status: **REQUEST_CHANGES** — P0 编译失败 5 处 + P0 关键基础设施未实现
---

# 对抗性代码审查报告：ce-executor-serial 机制层修复

> **审查原则**:本次审查以对抗性视角执行,不信任 commit message,逐行验证 destructure 完整性、字段传播链、命名准确性与端到端语义命中。已排查语义欺骗、隐藏副作用、边界漏洞和连锁反应风险。

## 审查概要

- **Commit/PR**: working tree 未提交,branch=`pittcat-dev`
- **审查范围**:
  - `crates/ralph-core/src/hat_handoff/gate.rs`（+188,5 个新测试 + GateDecision::Reject 加 `kind` 字段）
  - `crates/ralph-core/src/preset/engine/gates.rs`（+77,RejectionKind 加 3 个 variant + 测试）
  - `task.md`（-71,改动范围不可解释,详见 §"task.md 误改评估"）
- **总体结论**: **REQUEST_CHANGES**
- **风险等级**: **HIGH**
- **编译状态**: **FAIL**(2 个 E0027 pattern destructure 错误,在 ralph-core 阻断全 workspace)

---

## P0 - 阻断问题

### P0-1 [编译失败] 6 处 `GateDecision::Reject` match arm 缺少 `..`,ratchet-style enum 字段扩展未安全收尾

- **位置**:
  - `crates/ralph-core/src/event_loop/mod.rs:7435-7438`（destructure `{ reason_code, message }`,缺 `kind`） — **已确认编译失败**
  - `crates/ralph-core/src/preset/engine/gates.rs:210-213`（destructure `{ kind: rej.kind, message: rej.message }`,缺 `reason_code`）— **推断编译失败**
  - `crates/ralph-core/src/preset/engine/gates.rs:305-306`（测试代码,`{ kind, message }`,缺 `reason_code`）— **推断编译失败**
  - `crates/ralph-core/src/preset/engine/linter.rs:312-314`（`{ kind, message }`,缺 `reason_code`）— **推断编译失败**
  - `crates/ralph-core/src/event_loop/mod.rs:5143`（`{ kind, message }`,缺 `reason_code`）— **推断编译失败**
  - `crates/ralph-cli/src/policy_check.rs:582-585`（`{ reason_code, message }`,缺 `kind`）— **推断编译失败**
- **详细分析**:
  - 修复 worker 在 `gate.rs:75-79` 给 `GateDecision::Reject` 加了 `kind: RejectionKind` 字段,但只给 **`reject_to_task_resume`(行 358-361)** 一处加了 `..`。全仓 6 处下游 match 都没补 `..`,Rust 编译器在 `E0027 pattern does not mention field` 上严格 fail。
  - 修复 worker 自己提示了 1 处风险,实际**疏漏 5 处**。这是典型的"自我检查不充分"——grep 一次只搜到 1 处 match,没意识到 `RejectionKind` 在 5 个文件中跨 5 种 destructure 模式被消费。
  - 这不是"代码 bug",而是**"枚举扩展前的下游清点失败"**,违反 Rust 编译器保护的本质。
- **潜在影响**:
  - **整个修复不可用**:`cargo build` / `cargo check` / `cargo nextest` 全部在 ralph-core lib 这一步 fail。
  - CI 会立即红,任何 ralph 调度都拿不到新二进制。
  - 即使通过 `git stash` 回滚,30 天第 6 次复发的根因路径(诊断报告 P0-2)继续生效。
- **修复建议**:
  - 5 处 match arm **逐个加 `..`**:`Reject { kind: _, message, .. }` / `Reject { kind, message, .. }` / `Reject { reason_code, message, .. }`。
  - 推荐 5 处统一为 `Reject { reason_code, message, kind, .. }`(全字段 destructure)以减少未来扩字段再踩坑——或者改用 5 个 `..`。
  - 验证步骤:
    1. `cargo check --workspace --all-targets` 必须全绿。
    2. `cargo clippy --workspace --all-targets -- -D warnings` 必须全绿。
    3. `cargo nextest run -p ralph-core` 必须全绿,特别是 `engine_gates` 与 `hat_handoff::gate` 两个 mod 的 inline tests。
- **验证方式**:
  - 命令:`cargo check -p ralph-core --all-targets 2>&1 | grep -c "error\[E0027\]"` 必须 = 0。
  - 命令:`cargo check -p ralph-cli --all-targets 2>&1 | tail -5` 必须显示 `Finished` 而非 `error`。
  - 触发**二轮修复**:本报告认为这是阻断级疏漏,修复 worker 必须回来补完 5 处 + 验证编译。

### P0-2 [范围未达标] 修复描述承诺的"typed 升级链路"实际未实现,`RejectionKind` typed routing 是**死端**

- **位置**: `crates/ralph-core/src/hat_handoff/gate.rs:15-25`(docstring)、`gate.rs:55-67`(enum docstring)、`gate.rs:495-510`(测试 docstring)
- **详细分析**:
  - 修复在 docstring 中 6 次提到 `LoopState::record_typed_lint_rejection` / `consecutive_lint_rejections:{kind}` / "drift finding 升级",但:
    - **`record_typed_lint_rejection` 方法不存在**——`rg "fn record_typed_lint_rejection"` 全仓 0 命中。
    - **`consecutive_lint_rejections:{kind}` 按 kind 分桶的累加器不存在**——只有 `consecutive_engine_gate_rejections: u32`（单一计数,见 `crates/ralph-core/src/event_loop/tests/serial_lint.rs:274`）。
    - **drift_finding 升级**、**build_prompt 注入**、**circuit breaker 触发 plan.blocked** 这三件 plan 2026-06-21-001 U4 明确要做的核心机制,本次修复**完全没动**。
  - 注释/代码漂移是**隐藏的语义欺骗**——agent 读 fix 后的 `gate.rs` docstring 会以为升级链路已就绪,实际查 `loop_state.rs` 发现没有该字段、查 `event_loop/mod.rs` 没找到 caller、查 `recovery.jsonl` 升级路径未接——形成文档 vs 代码的认知陷阱。
  - 实际修复范围 = `RejectionKind` 加 3 个 variant + `GateDecision::Reject` 加 `kind` 字段 + 测试覆盖 kind 值。这是**typesafe 路由的基础**,但 plan 描述的"circuit breaker / drift_finding / prompt 注入"**一件都没做**。
- **潜在影响**:
  - 下一轮 fix worker / 排障 agent 看到 docstring 会误判修复已闭环,绕过 P0-2 真正的核心问题。
  - 修复 worker 没有生成 `docs/plans/2026-06-23-fix-ce-executor-serial-loop-orchestration-mechanism.md`(任务文档存在 `docs/plans/2026-06-23-003-cleanup-unified-state-legacy-remnants-plan.md` 但**没有本次 fix 的 plan**),**违反修复必须先有 plan 的硬规则**(参考 `task.md` 第 73 行"主 Agent 只做汇总和格式整理")。
- **修复建议**:
  1. **补 plan 文档**:`docs/plans/2026-06-23-fix-ce-executor-serial-loop-orchestration-mechanism.md`,明确本次实际范围(只做 typesafe 路由),并把 P0-2 剩余的 3 块(circuit breaker 计数 / drift_finding 升级 / prompt 注入)标记为 follow-up issue。
  2. **改 docstring**:把 6 处提到"record_typed_lint_rejection"的注释改为"本 PR 暂未实现;见 plan 2026-06-23-XXX follow-up",避免误导。
  3. **修本批 docstring 之后,跑一次反向验证**(CLAUDE.md 硬规则):用 `rg` 复查所有引用"record_typed_lint_rejection" / "consecutive_lint_rejections:{kind}"的注释,确保它们都明确标注"未实现"或删除。
- **验证方式**:
  - 命令:`rg "record_typed_lint_rejection" --type rust` 必须 0 命中(删除所有未来实现承诺的引用)。
  - 命令:`ls docs/plans/2026-06-23-fix-ce-executor-serial*.md` 必须存在本次 fix plan。

---

## P1 - 严重问题

### P1-1 [隐藏副作用] `Reject` 字段扩展后,`reject_to_task_resume` 的 `message: message.clone()` 在 typed 路径上变成"诊断信息但 routing 信息丢失"

- **位置**: `crates/ralph-core/src/hat_handoff/gate.rs:355-365`
- **详细分析**:
  - `reject_to_task_resume` 函数从 `Reject` 抽取 `(reason_code, message)` 构造 `task.resume`。修复后,函数**没有同步抽取 `kind`**,导致 `task.resume` payload 仍只有 reason_code 字符串。
  - 修复的真正意图(typed 路由)在此函数**完全没生效**——`task.resume` 消费者收到的是字符串 reason_code,无法按 kind 路由。
  - 这是**修复目标和实现脱钩的典型问题**。`event_loop/mod.rs:7435-7478` 那一大段"如果 reason_code 是 FILE_NOT_FOUND 则自动 regenerate"的逻辑,继续按字符串匹配——typed 分类的 4 倍精度(3 个 new kind + 1 个 HandoffArtifact)被这条路径完全屏蔽。
- **潜在影响**:
  - 诊断报告 P0-2 的 4 次 "hat_handoff_filename_mismatch" → 4 次 FILE_NOT_FOUND 之外,`task.resume` 消费者**永远拿不到 typed kind**,无法做 per-kind circuit breaker 计数,无法做 drift_finding 升级。
  - 修复的"typesafe routing"承诺,在这条热路径上**完全是空头支票**。
- **修复建议**:
  - `reject_to_task_resume` 函数签名应扩展为返回 `(&'static str, RejectionKind, String)`,或者把 `task.resume` payload 改为结构化 `Rejection { kind, reason_code, message }`。
  - 配合 docstring 澄清"本函数保留了字符串 reason_code 用于 `recovery.jsonl` 兼容;typed kind 留给 caller 走 `LintResumeHint` 路径"。
- **验证方式**:
  - 单元测试:构造一个 typed `Reject`,调用 `reject_to_task_resume`,断言返回值含 `kind` 字段(或 `task.resume` payload 显式带 kind 字段)。
  - 端到端测试:`recovery.jsonl` 中的 `retry_attempt` 字段应按 kind 独立累加,而非全局累加。

### P1-2 [测试欺骗] 3 个新测试 `*_carries_typed_kind` 只验证字段赋值,不验证 routing/counter 行为

- **位置**:
  - `crates/ralph-core/src/hat_handoff/gate.rs:509-531` `filename_seq_mismatch_carries_typed_kind`
  - `crates/ralph-core/src/hat_handoff/gate.rs:614-661` `structure_violation_carries_typed_kind`
  - `crates/ralph-core/src/hat_handoff/gate.rs:684-722` `illegal_emit_topic_carries_typed_kind`
- **详细分析**:
  - 三个新测试都只断言"evaluate_event 返回的 Reject 的 kind 字段是某个值",这是**字段赋值验证**,不是**端到端行为验证**。
  - 修复的核心承诺(typed routing 让 source hat 收到正确的 LintResumeHint)未在测试中验证:
    - 没有测试调用 `LintResumeHint::from_typed_rejection(..., kind, ...)` 后检查 `hint.target == LintResumeTarget::SourceHat`(vs 老的 `PlanGate`)。
    - 没有测试验证 `to_lint_class()` 把 3 个新 kind 映射到 `HandoffArtifact`(修复 worker 在 `gates.rs:460-475` 加了 `p0_2_hat_handoff_kinds_route_to_artifact_class` 测试,但**只在 gates.rs 单元内**——没在 `gate.rs` 与 `linter.rs` 集成层验证)。
    - 没有测试验证 typed 路径触发 circuit breaker / drift_finding 升级(因为这些机制根本未实现)。
- **潜在影响**:
  - 修复后即使 evaluate_event 输出的 kind 错误(比如把 `HandoffIllegalEmitTopic` 写成 `HandoffFilenameMismatch`),`cargo nextest` 仍会全绿——这是**测试覆盖的语义欺骗**。
  - 排障时,agent 看测试通过会以为修复完整,实际根本没验证 routing 行为。
- **修复建议**:
  - 在 `gate.rs` 测试里追加 `assert!(matches!(lint_hint.target, LintResumeTarget::SourceHat))`。
  - 在 `gates.rs` 现有的 `p0_2_hat_handoff_kinds_route_to_artifact_class` 之外,补一个 cross-file 集成测试:从 `evaluate_event` 输出 Reject → `LintResumeHint::from_typed_rejection` → 断言 routing 是 `SourceHat`。
  - 修正测试 docstring 措辞:`carries_typed_kind` 改为 `routes_typed_kind_to_source_hat`,更准确反映**目的**而非**手段**。
- **验证方式**:
  - 命令:`cargo nextest run -p ralph-core -- typed_kind | grep -c "PASS"` 至少包含上述 4 个测试(原 3 个 + 1 个新补的 routing 测试)。
  - 反向验证:把 gates.rs:113 的 `HandoffIllegalEmitTopic => LintFailureClass::HandoffArtifact` 临时改为 `LintFailureClass::TopicOwnership`,看测试是否 fail——目前应 fail(说明测试有覆盖率),但实际 fail 信息应明确指出"routing 错误"。

### P1-3 [测试盲区] `gates.rs` 新测试只验证 `reason_code()` 字符串,没验证 `to_lint_class()` 全 mapping

- **位置**:
  - `crates/ralph-core/src/preset/engine/gates.rs:411-428` `reason_code_for_new_kinds`
  - `crates/ralph-core/src/preset/engine/gates.rs:451-475` `p0_2_hat_handoff_kinds_route_to_artifact_class`
- **详细分析**:
  - `reason_code` 字符串已与历史 `recovery.jsonl` 一致(诊断报告 §4.5 的 `hat_handoff_filename_mismatch` 等字符串)——**SSOT 兼容性 ✓**。
  - 但测试只单独断言 `reason_code()`,没断言"reason_code 与 to_lint_class 的一致性"——如果有人误改 `reason_code` 但保持 `to_lint_class` 不变,字符串升级到 `recovery.jsonl` 后下游分析脚本会失效。
- **潜在影响**:
  - 字符串漂移:`recovery.jsonl` 的 reason_code 字符串与 preset 文案(`presets/en/ce-executor-serial.yml` 描述)、operational scripts(grep 脚本)、CLI help 文本可能漂移。
- **修复建议**:
  - 加一个测试断言:`for kind in ALL_KINDS { assert_eq!(kind.reason_code(), expected_strings[kind]) }` 一次性锁住 SSOT。
  - 把"string stability"约束写到 `RejectionKind::reason_code` 的 docstring(已部分做到,见 `gates.rs:120-122`),并显式引用"operators rely on this"。
- **验证方式**:
  - 命令:`rg "reason_code\(\) ==" --type rust` 应至少命中 8 处(原 5 + 新 3),覆盖每个 variant。

### P1-4 [行为漂移] `gates.rs:305-306` 的测试 `reject_when_required_missing` 在新 enum 下行为发生隐性变化

- **位置**: `crates/ralph-core/src/preset/engine/gates.rs:296-312`
- **详细分析**:
  - 老代码:测试断言 `Reject { reason_code, message }`——本测试从未要求 `reason_code` 字段存在(用的是 `..`)。
  - 修复后,`Reject` 加了 `kind` 字段,但这个测试**没有补 `kind` 断言**。如果有人修改 `run_gates` 把 `MissingField` 改成其他 kind,这个测试仍会通过——**这是修复的副作用未对测试产生 expected assertion pressure**。
- **潜在影响**:
  - 类似 `MissingField` 的 kind 改动可能不会被这个测试发现。
- **修复建议**:
  - 改成 `assert_eq!(kind, RejectionKind::MissingField)` 显式断言。
  - 同样,`gates.rs:347-352` 的 `reject_macro_edge_without_handoff_path` 测试也应显式断言 `kind == HandoffArtifact`。
- **验证方式**:
  - 命令:`rg "GateDecision::Reject \{ kind, message \}" --type rust` 应至少包含 4 处(每处都有显式 `assert_eq!` 验证 kind 值)。

---

## P2 - 建议

### P2-1 [命名误导] `RejectionKind::HandoffArtifact` 在新 typed 体系下粒度太粗,可能成为"垃圾桶枚举"

- **位置**: `crates/ralph-core/src/preset/engine/gates.rs:78-92`
- **详细分析**:
  - 修复后,3 个新 kind(`HandoffFilenameMismatch` / `HandoffStructureInvalid` / `HandoffIllegalEmitTopic`)都映射到 `LintFailureClass::HandoffArtifact`。
  - 老 `HandoffArtifact` kind 也映射到 `LintFailureClass::HandoffArtifact`。
  - 4 个 kind 共享同一个 lint class——这没问题(target 路由对),但**未来新增"agent 写了 handoff 但路径 escape"等更细 kind 时,粒度边界会变得模糊**。
  - 诊断报告 P0-2 已经识别"## notes 超 15 词"和"非法 topic"是两个独立失败模式——但 `to_lint_class` 把它们压平成同一 class,**未来按 kind 做 drift_finding 升级时,`HandoffArtifact` 这个"老 kind"会吸收所有新 typed 数据,导致升级粒度退化**。
- **修复建议**:
  - 把 `HandoffArtifact` 拆为 `HandoffMissing` / `HandoffPathEscape` / `HandoffFileReadFail`,让所有 5-6 个 kind 在 `to_lint_class` 之前保持正交。
  - 或者:明确文档说明"linter class 故意只到 HandoffArtifact 粒度,future plan U5b 会按 kind 拆分 class",并在 `to_lint_class` 注释里引用该 plan。
- **验证方式**:
  - 决策记录(ADR) 或 plan 文档,显式说明"为什么 4 个 kind 共享 1 个 class"。

### P2-2 [文档漂移] `gate.rs` 注释中 6 处提到不存在的方法/字段

- **位置**: `gate.rs:15-25, 55-67, 495-510, 605-617, 683-697`(均引用 `LoopState::record_typed_lint_rejection` 等)
- **修复建议**: 全部改为"本 PR 暂未实现 X,见 plan Y follow-up"或删除。

### P2-3 [test 范围] `gate.rs` 新增 3 个测试都基于"hypothetical downstream publishes"传字符串数组,没有验证"downstream publishes 来自 handoff_index 而非 hardcoded"

- **位置**: `gate.rs:687-701` 等
- **详细分析**:
  - 测试里 `let downstream = vec!["work.done".to_string(), "work.failed".to_string()];` 是硬编码——但生产代码应从 `HandoffIndex` 派生。如果 production 路径从 index 派生,会因 `index.consumer_of(topic)` 的兜底(`unwrap_or(from_hat)`)导致 routing 行为与测试预期不一致。
- **修复建议**: 测试 fixture 使用 `HandoffIndex::with_consumer("work.ready", "executor", &["work.done", "work.failed"])`,与 production 一致。

### P2-4 [魔法数字] `HandoffIndex::consumer_of` 的兜底逻辑可能掩盖 typing 错误

- **位置**: `crates/ralph-core/src/event_loop/mod.rs:7466-7469` `let consumer_hat = self.handoff_index.consumer_of(&ev.topic).unwrap_or(from_hat);`
- **详细分析**:
  - 修复后的 `HandoffIllegalEmitTopic` 检测依赖 `inputs.downstream_publishes` 字段——而 `event_loop/mod.rs` 没有传这个字段(测试路径里手动传)。**这意味着 production runtime gate 实际不会触发 illegal_emit_topic 检测**,只有 CLI `policy_check` 路径会触发。
  - 这是**修复的盲区**:lint 在 CLI 边界生效,但 runtime loop 没有走同一路径——`recovery.jsonl:4` 的失败在生产环境**完全不会触发**。
- **修复建议**:
  - 验证 `event_loop/mod.rs:7297-7430` 区域是否构造 `inputs.downstream_publishes`,如果未构造,补上(从 handoff_index 派生)。
  - 加测试覆盖 runtime 路径:`evaluate_event` 在 `downstream_publishes=[]` 时是否真的 reject(模拟 production 状态)。
- **验证方式**:
  - 命令:`rg "downstream_publishes" --type rust -n` 确认 production 路径确实传了该字段。

---

## 兼容性评估

### API 变更
- **`GateDecision::Reject` 新增 `kind: RejectionKind` 字段**(非 exhaustive enum 风险:`match` 模式必须补 `..`,见 P0-1)。
- **`RejectionKind` 新增 3 个 variant**: `HandoffFilenameMismatch` / `HandoffStructureInvalid` / `HandoffIllegalEmitTopic`(non_exhaustive 已加,需验证)。
- **公共函数签名未变**:`reject_to_task_resume` / `evaluate_event` 签名稳定。

### 数据格式变更
- **`.ralph/recovery.jsonl` 字符串 reason_code 兼容**(已确认:3 个新 kind 的 `reason_code()` 与诊断报告 §4.5 的字符串一一对应)。
- **`.ralph/events.jsonl` 业务事件 schema 不变**(只新增 `kind` 字段在内存 enum 中,不写入事件文件)。
- **`.ralph/agent/hat-handoff/*.md` 内容 schema 不变**(linter 检测的是文件内容,未改 validator)。

### 依赖变更
- **`crates/ralph-core`**: 影响 5 个文件(本仓 4 + ralph-cli 1),全部已 grep 验证。**ralph-api 无影响**(`rg RejectionKind` 在 ralph-api = 0 命中)。
- **`crates/ralph-cli/src/policy_check.rs:582-585`**: 需要补 `..`(见 P0-1 第 5 处)。
- **未同步检查 `crates/ralph-bench`**:`rg GateDecision crates/ralph-bench` 未跑,但 ralph-bench 是独立 benchmark,通常不消费 core 内部 enum——但仍需 fix worker 验证。

### 回滚安全性
- ✅ `git checkout -- crates/ralph-core/src/hat_handoff/gate.rs crates/ralph-core/src/preset/engine/gates.rs` 可干净回滚(纯新增,无 schema 破坏)。
- ⚠️ `task.md` 是用户输入的诊断 prompt,**不应被修改**,回滚后诊断 prompt 内容会恢复(但修改后的内容已丢失——见 P3 task.md 评估)。

### lint/clippy/fmt
- 未跑 `cargo fmt --check` / `cargo clippy -- -D warnings` / `cargo nextest`——编译失败导致这些全部 fail。
- 修复后必须跑 `cargo clippy --workspace --all-targets -- -D warnings` 确保无新 warning。

---

## 测试充分性评估

### 新增测试覆盖
- **field assignment 覆盖**:✓ 3 个新测试验证 `kind` 字段被正确赋值(只测**手段**,不测**目的**)。
- **`reason_code()` 字符串覆盖**:✓ 1 个新测试 + 1 个更新测试。
- **`to_lint_class()` mapping 覆盖**:✓ 1 个新测试 `p0_2_hat_handoff_kinds_route_to_artifact_class`。

### 缺失测试场景
- ❌ **集成测试**:从 `evaluate_event` → `LintResumeHint::from_typed_rejection` → 验证 `target == SourceHat` 的端到端流(见 P1-2)。
- ❌ **boundary test**:`downstream_publishes = []` 时是否仍触发 `HandoffIllegalEmitTopic` 检测(见 P2-4)。
- ❌ **回归测试**:`recovery.jsonl:1-4` 4 个原 case 复现,验证修复后能 typed classify 到正确的 kind(应作为 acceptance test)。
- ❌ **circuit breaker 联动测试**:因为本次未实现 `record_typed_lint_rejection` / `consecutive_lint_rejections:{kind}`,无法测试。
- ❌ **CLI `policy_check` 与 runtime gate 一致性测试**:同一 hat_handoff artifact 在 CLI 路径 vs runtime 路径下分类是否一致(诊断报告 P0-2 核心痛点)。

### 回归风险
- **高风险**:`Reject` 字段加字段后,所有 destructure 处都可能编译失败或行为退化(P0-1 已证)。
- **中风险**:`RejectionKind` 加 variant 后,任何 `match kind` 没补 `..` 的地方会编译失败(需 `rg "match.*kind" --type rust -n | head -20` 复查)。
- **中风险**:`to_lint_class()` 加新 arm 后,如果 `LintResumeTarget::from_class()` 兜底到某个 default,可能 routing 错位(需验证 `from_class` 的 exhaustive)。
- **低风险**:`reason_code()` 字符串与历史兼容(已确认)。

---

## task.md 误改评估

### 是否属于本次修复范围?
**否。** `task.md` 是工作区根的"诊断 prompt 模板"文件(由用户在 5-7 行注释中明确为 prompt),不属于 `ce-executor-serial` 机制层修复的目标。

### 改动分析
- **修改 1 (行 1)**: `/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/...` → `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph` —— 这是**路径修正**,合理(诊断报告指明 worktree 是 `ralph-e2e` 而非主仓 worktree)。
- **修改 2 (行 112-184)**: 删除"## 任务 3:老编排状态代码清零"整段(71 行)—— 这是 **P0-3 范围(老编排清理)已完成的工作,目前 task.md 残留**,修复 worker 顺手清掉了。

### 问题
1. **修复 worker 没有说明**: commit message / 任务摘要未提及 `task.md` 改动。
2. **范围越界**: `task.md` 是工作区根的辅助 prompt,不在 fix 范围内。
3. **删除未通知**: 删除"## 任务 3"会丢失该工作的可追溯性;应改为"该任务已完成,见 commit X"而不是直接删除。

### 修复建议
- **方案 A(推荐)**: 撤销 `task.md` 改动,`git checkout -- task.md`。
- **方案 B**: 保留修改,但在 fix 摘要中显式说明"同时清理 task.md 残留任务 3(已闭环,见 commit 69ca30f)"。
- 选 A 更稳妥,因为 task.md 是用户输入,不应被 worker 自动改。

---

## 对抗性审查声明

> 本审查基于对抗性原则执行,已排查以下风险维度:
>
> 1. **语义欺骗检测**: 发现修复 docstring 6 次引用不存在的方法 `LoopState::record_typed_lint_rejection` 与字段 `consecutive_lint_rejections:{kind}`(P0-2)。修复 worker 的"typesafe routing 完整闭环"承诺实际上是**部分闭环**——只是 enum 字段,没有升级链路。
> 2. **假设开发者犯错**: 修复 worker 自己提示了 1 处 destructure 风险,实际**漏修 5 处**(P0-1)。这说明"自我检查"在 Rust enum 扩展场景下不可靠,必须靠编译器兜底 + 全仓 grep 复查。
> 3. **连锁反应推演**:`Reject` 加字段 → 5 个文件的 6 处 match arm → 1 处 pre_check 转换 + 4 处 match + 1 处测试 → 全仓编译失败。
> 4. **极端条件注入**:`downstream_publishes = []` 时 production 路径可能绕过 `HandoffIllegalEmitTopic` 检测(P2-4)。
> 5. **回滚安全评估**: 代码部分可 `git checkout` 干净回滚;`task.md` 修改回滚后内容恢复,但 P0-2 真正的修复闭环仍未做——回滚 = 维持原状,不解决根因。
> 6. **隐式契约审查**:`reason_code` 字符串作为 `recovery.jsonl` SSOT 与 enum typed routing **双轨**——这一契约未在 plan 文档中显式声明,只在 docstring 提到,违反 CLAUDE.md "preset SSoT 多点同步"硬规则。
>
> **总体**: 修复 worker 完成了**基础设施第一步**(typed 路由),但**未完成修复闭环**(circuit breaker / drift_finding / prompt 注入),且**编译未通过**。**触发二轮修复**。

---

## 附录: 修复必须补完的 5 处 destructure(给修复 worker 的清单)

```rust
// 1. crates/ralph-core/src/event_loop/mod.rs:7435-7438
GateDecision::Reject {
    reason_code,
    message,
    kind,  // ← 新增
} => { ... }

// 2. crates/ralph-core/src/event_loop/mod.rs:5143
GateDecision::Reject { kind, message, .. } => {  // 已有,验证编译
    // ...
}

// 3. crates/ralph-core/src/preset/engine/linter.rs:312
GateDecision::Reject { kind, message, .. } => {  // 已有,验证编译
    LintOutcome::Reject(LintResumeHint::from_typed_rejection(topic, kind, &message))
}

// 4. crates/ralph-core/src/preset/engine/gates.rs:210-213
return GateDecision::Reject {
    kind: rej.kind,
    message: rej.message,
    reason_code: rej.kind.reason_code(),  // ← 新增
};

// 5. crates/ralph-core/src/preset/engine/gates.rs:305-306 (test)
match decision {
    GateDecision::Reject { kind, message, .. } => {  // ← 加 ..
        assert_eq!(kind, RejectionKind::MissingField);
        assert!(message.contains("plan_name"));
    }
    // ...
}

// 6. crates/ralph-cli/src/policy_check.rs:582-585
GateDecision::Reject {
    reason_code,
    message,
    kind: _,  // ← 或 .. 
} => Err(ValidationError { ... })
```

**反向验证命令**(修复后必须全绿):
```bash
cargo check --workspace --all-targets 2>&1 | grep -c "error\[E0027\]"  # 必须 = 0
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3   # 必须 Finished
cargo nextest run -p ralph-core -- hat_handoff::gate 2>&1 | tail -10   # 必须全 PASS
cargo nextest run -p ralph-core -- engine_gates 2>&1 | tail -10       # 必须全 PASS
```

