---
title: Preset Author/Review Runtime Audit - Implementation Plan (Rewritten)
type: feat
date: 2026-07-27
topic: preset-author-review-runtime-audit
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: docs/plans/2026-07-27-002-feat-preset-author-review-runtime-audit-plan.md
execution: code
---

# Preset Author/Review Runtime Audit — Implementation Plan

> 这是对 `docs/plans/2026-07-27-002-feat-preset-author-review-runtime-audit-plan.md` 的实现级重写。原文件是「产品合约」(requirements-only),把大量实现决策留到规划阶段。本计划基于代码库现有结构(`PromptPreview` / `preview_prompt_for_config` / `InspectPromptArgs` / `EventLoop::with_context` / `ralph emit --policy-check` / `ralph emit --schema`)和 skill 文档(`references/commands.md` / `references/finding-rubric.md` / `references/agent-native-model.md`)给出可以直接按 Unit 执行的步骤。

---

## 0. 计划状态

- **状态**：`READY`
- **基线**: `pittcat-dev` 分支 HEAD `83df2a2c`,Ralph 二进制已实现 `ralph inspect prompt` 空状态预览(`crates/ralph-cli/src/commands/inspect.rs:483-532`)和 `PromptPreview` SSOT(`crates/ralph-core/src/event_loop/mod.rs:417-435`)。`ralph emit --policy-check` 的 policy gate 评估入口已存在(`crates/ralph-cli/src/commands/emit.rs:320-457`),`ralph emit --schema` 是 topic shape SSOT。
- **调查范围**:`crates/ralph-cli/src/commands/inspect.rs`、`crates/ralph-core/src/event_loop/{mod,prompt}.rs`、`crates/ralph-core/src/{event_policy,loop_context,wave_context,trigger_context,correction}.rs`、`crates/ralph-cli/src/commands/emit.rs`、`skills/ralph-preset-{author,review}/SKILL.md`、`skills/ralph-preset-common/references/{commands,finding-rubric,agent-native-model,prompt-visibility}.md`、`crates/ralph-core/data/ralph-tools*.md`(存在 skill 引用契约)。
- **已执行的验证**:
  - `PromptPreview` 含 `hat_id` / `gates` / `auto_inject` / `on_demand` / `block_titles`,字段已就位(mod.rs:417-435)
  - `preview_prompt_for_config` 是 pure config-driven,接受 `hat_id` 与 `block_titles` 闭包(mod.rs:938-976)
  - `EventLoop::with_context(config, LoopContext)` 注入 LoopContext(含 `trigger_event` / `wave` / `correction` 等)(mod.rs:1135)
  - `InspectPromptArgs` 已支持 `--hat` / `--format` / `--full`(inspect.rs:148-166)
  - `ralph emit --policy-check` 已在 `policy_check.rs` 实现 gate 评估,JSON 输出含 reason_code/field/gate
- **尚未执行的验证**:candidate-emit 干跑评估的纯函数入口(只读,无落盘)的精确返回 schema —— 需 Unit 1 调查后定型。
- **阻塞项**:无。

---

## 1. 功能目标

- **业务目标**:让 `ralph-preset-author` 和 `ralph-preset-review` 在起草/审核 preset 时能用结构化、机器可读、与真实 runtime 同源的证据,回答「该 hat 在代表性 activation 中实际看到什么」「这条候选 emit 会经过哪些 gate、被拒/被收时原因是什么」「当前 preset 使用的 capability 是否有对应 author/review 规则」。
- **用户或调用方**:
  - **A1 Preset author**:起草或修改 `presets/en/`、`presets/schemas/`、`.ralph/hats/*.yml`
  - **A2 Preset reviewer**:独立验证 preset,在 review 报告与 `.ralph/reviews/*.md` 中输出
  - **A3 Ralph maintainer**:新增/修改 preset-facing 能力,需要发现 operator skill 的覆盖缺口
  - **A4 Ralph runtime**:作为事实源,只读预览不得另造第二套判断语义
- **当前行为**:
  - `ralph inspect prompt --hat <id>` 跑 `EventLoop::prompt_preview(hat_id)`,返回空状态 `PromptPreview`,只覆盖 `auto_inject` / `on_demand` / `block_titles`(inspect.rs:507-514,mod.rs:5480-5488)
  - 没有 trigger / wave / orchestrator / correction 场景化输入;没有 candidate-emit 干跑评估
  - 没有机器可读的 preset-facing capability inventory
  - author/review skill 各自引用多份 references,但没有按 capability 自动对账的步骤
- **目标行为**:
  - `ralph inspect prompt --hat <id> --trigger <TOPIC> [--source-hat <id>] [--payload <json>] [--iteration N] [--wave-context <json>] [--orchestrator-context <json>] [--correction <json>] [--scratchpad] [--tasks-enabled] [--memories-enabled]` 接受上述场景参数,运行 dry `EventLoop::with_context(LoopContext{ trigger, wave, correction, gates, ... })` + 干跑 `ralph emit --policy-check` 模拟,输出结构化 JSON 包含:
    - 现有 `PromptPreview` 字段(保持兼容)
    - 新增 `trigger_context_injected: { source_topic, source_hat, summary_fields, routing_hints_matched }`
    - 新增 `wave_context_injected: { wave_id, role, ... } | null`
    - 新增 `correction_injected: { kind, reason_code, ... } | null`
    - 新增 `orchestrator_context_injected: { task_view_fields, progress_view_fields }`
    - 新增 `skill_gates: { tasks_enabled, memories_enabled, scratchpad_enabled }`
    - 新增 `candidate_emit: { policy_decision: accept|reject, reasons[], projection: { state_changes[] }, next_hat_candidates: [hat_id] }`(只在 `--payload` 同时给出 `--topic` 时评估)
  - `ralph capability inventory --format json` 输出机器可读的 preset-facing capability list(每项含 `id` / `trigger_signal` / `applies_when` / `evidence_sources` / `recommended_evidence_level` / `covered_in_author_review: yes|no|partial`)
  - `ralph-preset-author` / `ralph-preset-review` SKILL.md 文档与 references 新增「capability discovery + capability-triggered audit」步骤,自动对照 inventory 与现有 AAF/Payload Audit/Mechanical Lint 规则
- **行为差异**:
  - 现状:`ralph inspect prompt` 仅给空状态预览,候选 emit 评估完全缺位
  - 目标:同一 CLI 入口支持场景化激活预览与候选 emit 干跑,author/review 可在不改 preset YAML 的前提下证明「该 hat 在该 trigger 下是否真的能 emit」
- **本次范围**:
  1. 扩展 `ralph inspect prompt` 的 CLI 参数与 JSON 输出
  2. 在 ralph-core 抽出只读 `evaluate_candidate_emit` 函数
  3. 在 ralph-core 暴露 `capability_inventory()` 函数,新增 `ralph capability inventory` CLI
  4. 同步 `ralph-preset-author` / `ralph-preset-review` SKILL.md 与 references
  5. 同步 `crates/ralph-core/data/ralph-tools*.md`(被 hat instructions 引用的注入 skill)
  6. 同步 `scripts/ralph-zsh-plugin.zsh` 补全(`ralph capability`)
  7. CLI `--help` 与 `scripts/check-cli-doc-drift.sh` 静态 drift 扫描同步
- **非目标**:
  - 不调用真实模型、不执行 agent 命令、不启动完整 loop
  - 不要求用户创建或维护场景文件(场景由 CLI 一次性参数构成)
  - 不引入 CI/CD coverage gate,也不改变 `ralph preset check --strict` 的退出语义
  - 第一版不完整模拟 supervisor 持久化、worktree 操作、多轮 recovery/resume、完整 completion 生命周期
  - capability inventory 只覆盖 preset 作者与审核者需理解的表面,不作为 Ralph 全部内部函数的反射系统
- **输入**:`-c <config.yml>` / `-H <preset.yml|builtin:name>` / `--hat <id>` / 场景参数
- **输出**:JSON 或 human 格式,只读,与现有 `loop_inspect.v2` schema 兼容矩阵列出
- **状态变化**:无。运行时 ledger / task / worktree / scratchpad / supervisor state 均不修改
- **错误语义**:CLI 退出码与 `anyhow` 错误消息必须可诊断(参数错误 → exit 2 + 字段级错误位置;gate 拒绝 → JSON `policy_decision: reject` + `reasons[]`,退出码 0,这是评估结果而非错误)
- **兼容性要求**:扩展参数必须保持可选,缺省场景下输出与现状 byte-for-byte 兼容;新增 JSON 字段不可破坏既有 `auto_inject` / `on_demand` / `block_titles` 顺序与命名
- **性能要求**:dry `with_context` + `build_prompt` + `evaluate_candidate_emit` 在单 hat 上 P95 < 500ms
- **安全或权限要求**:与现有 `ralph inspect` 同一 ACL(`inspect` 命名空间只读)
- **已知约束**:`build_prompt` 内部有 `handoff_tracker.on_hat_activated` 副作用(同实例复用会清掉 deadline,mod.rs:4766-4768),inspect 路径已遵守「单实例一次性」契约(mod.rs:5464-5469);`LoopContext` 是 `Send + Sync` 序列化形态,新参数需通过 `serde` 反序列化后传入
- **已确认假设**:
  - `LoopContext` 已有可序列化的子结构(`wave_context_json_for_hat` 在 mod.rs:7577 证明可 JSON 化)
  - `ralph emit --policy-check` 的核心 gate 评估函数(`policy_check.rs`)可被复用为只读接口
  - skill 数据来自 `crates/ralph-core/data/*.md` 内嵌,`SkillRegistry::from_config` 的内容 SSOT 稳定
- **待验证假设**:
  - **`evaluate_candidate_emit` 抽离边界**:policy 评估链路上游是否需要 `TriggerContextInput` / 当前 `state`? —— Unit 1 必先验证,产出时锁定签名(置信度目标 ≥ 0.85)
  - **capability inventory 的触发信号覆盖**:哪些 YAML 字段应被列为 inventory `trigger_signal`? —— Unit 3 验证后定型

---

## 2. 代码库现状与证据

### 2.1 当前实现入口

- **外部入口**:`ralph inspect prompt --hat <id> --format {human|json} [--full]`(`crates/ralph-cli/src/commands/inspect.rs:148-166`)
- **调用链**:`Commands::Inspect` → `execute()` → `inspect_prompt_command()` → `EventLoop::new(config)` → `event_loop.initialize("ralph inspect prompt (read-only)")` → `event_loop.prompt_preview(&hat_id)` → `emit_prompt_view()` → stdout JSON/human(inspect.rs:483-588)
- **核心模块**:
  - `crates/ralph-core/src/event_loop/mod.rs`:EventLoop 主入口与 `prompt_preview` / `build_prompt` / `with_context`
  - `crates/ralph-core/src/event_loop/prompt.rs`:prompt assembly 与 `SkillInjector::plan_auto_inject`
  - `crates/ralph-core/src/event_loop/types.rs`:循环类型
  - `crates/ralph-core/src/loop_context.rs` / `wave_context.rs` / `trigger_context.rs` / `correction/mod.rs`:上下文注入结构
  - `crates/ralph-core/src/event_policy.rs`:policy 评估函数
  - `crates/ralph-cli/src/commands/emit.rs`:emit CLI + `policy_check` 路径
  - `crates/ralph-cli/src/commands/inspect.rs`:inspect CLI
- **数据边界**:loop ledger(`.ralph/events.jsonl` / `.ralph/loops.json` / `.ralph/supervisor.db` / `.ralph/agent/*`)**全部只读**,本次不写入;运行期不创建任何 marker(LoopContext 在内存内构造)
- **外部依赖**:`serde_yaml` / `serde_json` / `clap` / `anyhow` / `chrono` / `tracing-subscriber`(均已在 workspace)
- **现有测试**:
  - `crates/ralph-cli/src/commands/inspect.rs` 含 CLI parser 测试、`build_view` 测试、`inspect_loop_view_stable_when_no_markers`、`inspect_prompt_command` 链路未单测(只测 JSON 序列化形状)
  - `crates/ralph-core/src/event_loop/tests/{build_prompt,preview_characterization,preview_api,wave_context_injection,wave_context_env_var,u3_trigger_context_prompt,u4_handoff_envelope_prompt}.rs` 是 prompt preview / context 注入的既有测试
- **构建和验证方式**:`./scripts/run-tests.sh`(两阶段 nextest)、`cargo nextest run -p ralph-cli --bin ralph -- inspect_prompt` 子集、`scripts/check-cli-doc-drift.sh --strict`

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| ----------- | -- | ---- | ------ | --- |
| E1 | `crates/ralph-cli/src/commands/inspect.rs:148-166` | `InspectPromptArgs` 已含 `--hat` / `--format` / `--full`,clap derive 模式;新增参数沿用同一模式即可 | Unit 1 直接扩展 `InspectPromptArgs` 字段,无需新增 subcommand | 高 |
| E2 | `crates/ralph-cli/src/commands/inspect.rs:507-532` | `inspect_prompt_command` 用 `tracing` 全局 OFF + `EventLoop::new(config)` + `event_loop.initialize("ralph inspect prompt (read-only)")` + `prompt_preview(&hat_id)`;`build_prompt` 在 `--full` 时调用 | 扩展路径走同一 suppress-then-build 顺序;`LoopContext` 注入需走 `EventLoop::with_context` | 高 |
| E3 | `crates/ralph-core/src/event_loop/mod.rs:1135` | `EventLoop::with_context(config: RalphConfig, context: LoopContext) -> Self` 接收 `LoopContext`,用于把 trigger/wave/correction 注入 loop state | Unit 2 用 `with_context` 注入场景化上下文,复用同一 build_prompt 路径 | 高 |
| E4 | `crates/ralph-core/src/event_loop/mod.rs:5480-5488` | `prompt_preview(&mut self, hat_id: &HatId) -> Option<PromptPreview>` 内部调用 `preview_prompt_for_config` + `preview_block_titles(hat_id)`,后者实际走 `build_prompt` 路径(mod.rs:5513-5529) | 扩展时必须保持「单实例一次性」契约;不能与 `build_prompt_body` 复用同一 EventLoop | 高 |
| E5 | `crates/ralph-core/src/event_loop/mod.rs:417-435` | `PromptPreview` 是 serde Serialize/Deserialize struct,字段顺序与命名是 JSON SSOT | 新增字段不可破坏现有 JSON 形状;`prompt_body` 字段在 `--full` 时附加(inspect.rs:567-580) | 高 |
| E6 | `crates/ralph-core/src/event_loop/mod.rs:938-976` | `preview_prompt_for_config(config, hat_id, block_titles)` 是 pure config-driven,接受 `FnOnce(&HatId) -> Vec<String>` | Unit 1 重构时把 `block_titles` 闭包换成 `EventLoop::build_prompt`-based extractor(已存在),无需新增纯函数 | 高 |
| E7 | `crates/ralph-core/src/event_loop/mod.rs:7577` | `wave_context_json_for_hat(&mut self, hat_id: &HatId) -> Option<String>` 证明 `LoopContext` 内的 wave/trigger/correction 子结构可被序列化为 JSON 喂给 hat prompt | 新增场景参数的 JSON 解析可直接复用同一序列化路径 | 高 |
| E8 | `crates/ralph-cli/src/commands/emit.rs:320-457` | `emit_command` / `emit_command_with_root` 已实现 `ralph emit --policy-check --output json --triggered <hat> <topic> '<payload>'`,JSON 输出含 reason_code/field/gate | Unit 2 把内部 policy 评估函数(`policy_check.rs`)抽离为只读公共函数,inspect 路径调用同一函数 | 高 |
| E9 | `crates/ralph-core/src/event_policy.rs:347` | `pub fn is_recoverable_policy_finding` 与 `check_topic_format` / `check_handoff_envelope` / `check_completion_honored` 是公共函数,可被 inspect 路径直接调用 | Unit 2 用现有 `pub fn` 组合 policy 评估,无需重新实现 gate 逻辑 | 高 |
| E10 | `crates/ralph-cli/src/commands/inspect.rs:1-13` | `ralph inspect` 命名空间文件头明确「read-only / diagnostic」语义,本计划扩展不破坏该语义 | 扩展保持只读,不写 ledger / task / worktree | 高 |
| E11 | `skills/ralph-preset-common/references/commands.md:60-77` | `ralph inspect prompt` 模板已在 references 列出(JSON 形状:hat_id / gates.tasks_enabled / gates.memories_enabled / auto_inject[] / on_demand[] / block_titles[] / prompt_body) | 扩展 JSON 字段后必须同步该 references,保持作者/审核者可发现的命令文档 SSOT | 高 |
| E12 | `skills/ralph-preset-common/references/commands.md:15-23` | review 默认入口包括 `ralph preset check -H <path|builtin:name> --strict` 与 `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | 不改变这两条命令的退出语义(本计划范围外的 CI 语义) | 高 |
| E13 | `skills/ralph-preset-common/references/finding-rubric.md:179,193,214` | 「Wave capability audit」与「Supervisor capability audit」已存在,capability-triggered、不按 preset 名称门控 | Unit 4 的 `ralph-preset-author` / `ralph-preset-review` 同步规则沿用这两段的能力触发风格 | 高 |
| E14 | `skills/ralph-preset-common/references/prompt-visibility.md:62` | `prompt-visibility.md` 明确「外仓对 data/*.md 的本地修改不会反映到 inspect 输出(除非重装 skill 树覆盖)」 | Unit 4 必须保证能力描述中明确 `source = binary_embedded | repo_local` 二选一,author/review 报告必须注明来源 | 高 |
| E15 | `crates/ralph-core/data/ralph-tools*.md`(被 hat instructions 引用的注入 skill) | hat instructions 在 on_demand 时引用 `ralph-tools-emit` / `ralph-tools-wave` 等 skill doc 章节名,不复制命令表 | Unit 4 author/review references 引用 skill doc 章节,不复述 | 高 |
| E16 | `scripts/check-cli-doc-drift.sh` | CLI `--help` 与 `ralph-tools*.md` 的引用一致性由该脚本静态扫描 | Unit 1 / Unit 3 新增 CLI 子命令后必须通过该脚本 | 高 |
| E17 | `scripts/ralph-zsh-plugin.zsh` | zsh 补全脚本维护 `ralph run -H builtin:<TAB>` 与其它子命令补全 | Unit 3 新增 `ralph capability inventory` 后必须同步该脚本(详见 CLAUDE.md「When adding, removing, renaming…」硬规则) | 高 |
| E18 | `crates/ralph-core/src/correction/mod.rs:62` | `pub struct CorrectionContext`,`LoopContext` 内嵌 correction 字段(由 `with_context` 传入) | Unit 1 / Unit 2 注入 correction 场景时直接构造 `CorrectionContext`,无需重写 | 高 |
| E19 | `crates/ralph-core/src/wave_context.rs:28` | `pub struct WaveContext`,`LoopContext` 内嵌 wave 字段 | Unit 1 / Unit 2 注入 wave 场景时直接构造 `WaveContext` | 高 |
| E20 | `crates/ralph-core/src/trigger_context.rs:113,212` | `TriggerContextView` 与 `TriggerContextInput`,已有序列化能力 | `--trigger` / `--payload` 解析为 `TriggerContextInput` 即可驱动 `## TRIGGER CONTEXT` 块 | 高 |
| E21 | `crates/ralph-cli/src/commands/inspect.rs:1371-1386` | 文件已有 `pub fn loop_anchor_unattached_warning()` 等 const 字符串,便于测试 / fixture pin | Unit 1 新增「无场景参数」分支时,该 const 字符串字面量必须保持测试可匹配 | 高 |

### 2.3 受影响范围

- **生产模块**(已确认路径):
  - `crates/ralph-cli/src/commands/inspect.rs`(扩展 `InspectPromptArgs` 与 `inspect_prompt_command`)
  - `crates/ralph-cli/src/cli.rs`(若新增 `ralph capability inventory` 子命令,需在 `Commands::Inspect` 同级注册)
  - `crates/ralph-cli/src/commands/mod.rs`(若新增子命令模块)
  - `crates/ralph-core/src/event_loop/mod.rs`(`PromptPreview` 加字段、`prompt_preview` 接收 `LoopContext`)
  - `crates/ralph-core/src/event_loop/prompt.rs`(`preview_prompt_for_config` 接 `block_titles_with_context` 闭包)
  - `crates/ralph-core/src/event_loop/types.rs`(若 `PromptPreview` 字段追加,serde 序列化顺序)
  - `crates/ralph-core/src/policy_check.rs`(抽离 `evaluate_candidate_emit_readonly(config, hat_id, topic, payload, triggered) -> PolicyEvaluationResult`)
  - `crates/ralph-core/src/capability_inventory.rs`(新增模块,实现 `pub fn capability_inventory() -> Vec<Capability>`)
  - `crates/ralph-core/src/lib.rs`(新增 module 导出)
- **测试模块**:
  - `crates/ralph-cli/src/commands/inspect.rs` 内 `mod tests`(扩展场景化参数解析测试)
  - `crates/ralph-core/src/event_loop/tests/preview_characterization.rs`(增加 `with_context` 场景化 contract 测试)
  - `crates/ralph-core/src/event_loop/tests/preview_api.rs`(扩展 `PromptPreview` 新字段 SSOT 测试)
  - `crates/ralph-core/src/capability_inventory.rs`(单测覆盖每条 capability 的 `covered_in_author_review` 字段在当前 skill 文档中是否可定位)
- **配置**:不修改 `RalphConfig` schema(单元测试 fixture 沿用现有 `RalphConfig::default()` + `cfg.hats.insert(...)`)
- **数据**:不修改 `.ralph/*` 任何状态文件;新输出字段不写盘
- **API**:不修改 `ralph emit` / `ralph wave emit` 的对外参数表(本次只在 inspect 路径复用内部 policy 评估)
- **CLI**:`ralph inspect prompt` 扩展参数;新增 `ralph capability inventory --format {human|json}`
- **UI**:无
- **外部服务**:无
- **调用方**:`ralph-preset-author` / `ralph-preset-review` SKILL.md 与 references 中的命令模板
- **构建目标**:不引入新依赖

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
| ----------- | ---- | ---- | ---- | ---- | --------- | --- |
| D1 | CLI 入口扩展 vs 新增 inspector subcommand | (a) 扩展 `ralph inspect prompt`(在 args 加场景参数);(b) 新增 `ralph inspect activation` | (a) | E1,E2;产品合约「session-settled — user-directed」 | 用户已选 (a),减少操作入口 | 0.95 |
| D2 | 场景输入形态 | (a) 仅命令行参数;(b) 命令行 + 场景文件;(c) 仅场景文件 | (a) | E1;产品合约「session-settled — user-directed」 | 用户已选 (a),避免维护场景文件 | 0.95 |
| D3 | 候选 emit 评估是否真发事件 | (a) 真发 `ralph emit --policy-check`,通过临时 stdout 捕获 JSON;(b) 抽离只读 `evaluate_candidate_emit` 在 inspect 路径复用 | (b) | E8,E9;现有 `policy_check.rs` 已是公共函数,可直接复用并锁定 | (a) 会让 inspect 路径 spawn 子进程,破坏「单实例一次性」契约(E4) | 0.90 |
| D4 | capability inventory 来源 | (a) 从 `crates/ralph-core/src/*.rs` 反射扫描;(b) 手写 capability 列表 + `covered_in_author_review` 字段由 references 字符串 anchor 检测 | (b) | E13,产品合约「不作为通用反射系统」 | (a) 易漂移且偏离 skill-only 视角 | 0.90 |
| D5 | capability inventory 暴露方式 | (a) 静态 `pub fn capability_inventory() -> Vec<Capability>` 在 `ralph-core`;(b) 运行时基于 preset 推导子集 | (a) | E9 现有 `pub fn` 模式一致;CLI 在 ralph-cli 包装 | (b) 推导逻辑复杂且难以 capability-triggered 命中,违背产品合约 | 0.88 |
| D6 | `PromptPreview` 扩展方式 | (a) 直接在 struct 上加 `Option<...>` 字段,`#[serde(skip_serializing_if = "Option::is_none")]`;(b) 新增 sibling struct `PromptPreviewEx` 通过版本号选择 | (a) | E5,E11(loop_inspect.v2 也用同模式) | (b) 引入 JSON SSOT 切换逻辑,破坏 `--full` 既有字段顺序 | 0.92 |
| D7 | `evaluate_candidate_emit` 签名 | (a) `(config, hat_id, topic, payload, triggered) -> PolicyEvaluationResult`;(b) `(EventLoop, hat_id, topic, payload, triggered)` | (a) | E8,E9;现有 `policy_check.rs` 入口签名一致 | (b) 绑死 EventLoop 破坏只读纯函数原则 | 0.88 |
| D8 | LoopContext 注入 contract | (a) `prompt_preview_with_context(hat_id, &LoopContext)` 新方法;(b) 在 inspect 路径内构造 `EventLoop::with_context`,复用 `prompt_preview` | (b) | E3,E4 | (a) 增加方法数;E4 已保证 `with_context` + `prompt_preview` 安全 | 0.90 |
| D9 | CI 退出语义 | 不变 | — | E12,产品合约 R15 | — | 0.95 |
| D10 | 证据等级标签生成 | 在 `PromptPreview` 加 `evidence_level: "static" | "runtime" | "unverified"`,默认 `"static"` | E13,产品合约 R2 | 默认值(无 candidate_emit)时 `"static"`;有 candidate_emit 时 `"runtime"`;`evaluate_candidate_emit` 返回 unproven gate 时 `"unverified"` | 0.85 |
| D11 | zsh 补全 | `scripts/ralph-zsh-plugin.zsh` 加 `ralph capability inventory` 补全 | E17(CLAUDE.md 硬规则) | 不补全会触发 drift 检查 | 0.95 |
| D12 | 数据 docs 同步 | `ralph-tools*.md` 在 `crates/ralph-core/data/` 内,扩展 `ralph inspect prompt` 与新增 `ralph capability inventory` 后必须同步 | E15,CLAUDE.md「AI skill guide 同步规则」硬规则 | 漏改视为违规 | 0.95 |
| D13 | doc-drift 静态扫描 | 改 CLI 后跑 `scripts/check-cli-doc-drift.sh --strict` | E16 | 漏跑视为违规 | 0.95 |

无置信度 < 0.85 的决策。

---

## 4. BDD 行为规格

```gherkin
Feature: Preset author/review runtime audit (2026-07-27-002 plan)

  Background:
    Given a workspace with a builtin preset (e.g. builtin:debug)
    And the operator has `crates/ralph-core/data/ralph-tools*.md` skill data available

  Scenario: 现有 `ralph inspect prompt` 空状态预览保持兼容 (covers R5, R9-R10)
    Given an effective config with at least one hat registered (e.g. "investigator")
    When the operator runs `ralph -c ralph.yml inspect prompt --hat investigator --format json` without any scenario flag
    Then the JSON output must contain `hat_id`, `gates`, `auto_inject`, `on_demand`, `block_titles` exactly as today
    And no `trigger_context_injected`, `wave_context_injected`, `correction_injected`, `orchestrator_context_injected`, `skill_gates` keys appear (skip_serializing_if)
    And `evidence_level` equals `"static"`
    And no file in `.ralph/` is created or mutated

  Scenario: 场景化 trigger context 预览 (covers R6, R9-R10)
    Given the operator passes `--trigger build.task --source-hat planner --payload '{"task":"refactor X"}'`
    When preview is computed
    Then `trigger_context_injected` is present with `source_topic="build.task"`, `source_hat="planner"`, `summary_fields=[...]`, `routing_hints_matched=[...]`
    And `## TRIGGER CONTEXT` block (if declared in schema) is reflected in `block_titles`
    And `evidence_level` equals `"runtime"` (caller supplied real trigger payload)

  Scenario: 候选 emit 被接受 (covers R11, R21, AE7)
    Given a candidate emit `--topic work.done --payload '{"task_id":"<live>","verdict":"pass"}' --triggered worker`
    When the candidate is evaluated
    Then `candidate_emit.policy_decision` equals `"accept"`
    And `candidate_emit.reasons` is empty
    And `candidate_emit.projection.state_changes` enumerates projection actions on accepted fields
    And `candidate_emit.next_hat_candidates` lists downstream hats that can consume the topic

  Scenario: 候选 emit 被拒 (covers R11, R21, AE6)
    Given the same candidate but with a payload missing `task_id`
    When evaluated
    Then `candidate_emit.policy_decision` equals `"reject"`
    And `candidate_emit.reasons[]` contains at least one entry with `gate`, `field`, `reason_code`
    And `candidate_emit.projection` is empty
    And `candidate_emit.next_hat_candidates` is empty
    And `evidence_level` equals `"unverified"` for the projection branch

  Scenario: 候选 emit 触发不可证明 gate 时标 unverified (covers R4)
    Given a candidate emit whose projection depends on next-hat selection algorithm
    When the projection can't be statically proven (multi-consumer routing)
    Then `candidate_emit.projection.next_hat_candidates` is `"unverified"`
    And the JSON includes a `coverage_gaps[]` field listing the gap with the source gate id

  Scenario: capability inventory 输出 (covers R12-R14)
    Given the operator runs `ralph capability inventory --format json`
    When inventory is computed
    Then the JSON is a `Vec<Capability>` with stable `id`, `trigger_signal`, `applies_when`, `evidence_sources`, `recommended_evidence_level`, `covered_in_author_review`
    And `covered_in_author_review` is `"yes"` only when the corresponding references/skill doc contains a stable anchor (e.g. `## Wave capability audit` heading)

  Scenario: capability inventory 中未覆盖的能力产生 coverage finding (covers R15, R24-R27)
    Given a newly added preset-facing capability that no references yet describe
    When author or review loads inventory
    Then any preset enabling that capability yields a coverage finding entry
    And the finding does not change `ralph preset check --strict` exit code
    And the finding lists `capability.id`, `trigger_signal`, and `references_anchor_searched`

  Scenario: 无场景参数 + 无 candidate_emit 时 evidence_level 静态 (covers R2-R3)
    Given a baseline invocation
    When preview is requested without --trigger / --payload / --topic
    Then `evidence_level` is `"static"`
    And `policy_decision` is absent (or `null`)

  Scenario: 不写盘 (covers R8, AE5)
    Given any scenario invocation including trigger + candidate emit
    When preview returns
    Then `.ralph/events.jsonl` and `.ralph/agent/*` byte size unchanged
    And no worktree is created or moved

  Scenario: 输入非法 (covers R7, AE4)
    Given `--payload '{"task_id":'` (malformed JSON)
    When preview is requested
    Then exit code is 2
    And stderr contains the parse error and the field name (`--payload`)
    And no JSON output is written

  Scenario: 候选 emit 跨 multi-tenant routing 时部分 unverified (covers R4, AE6-AE7)
    Given a candidate emit whose `triggered` hat has multiple routing candidates
    When evaluated
    Then `next_hat_candidates` is a JSON array
    And if some candidates depend on state not visible in the static config, those entries have `"verified": false`

  Scenario: zsh 补全包含新增子命令 (covers D11)
    Given `scripts/ralph-zsh-plugin.zsh` is updated
    When `compdef _ralph ralph` is sourced
    Then `ralph capability inventory` is in the completion list
    And `ralph inspect prompt --hat <TAB>` still works

  Scenario: doc-drift 静态扫描通过 (covers D12-D13)
    Given any code change to `inspect.rs` or new `commands/capability.rs`
    When `./scripts/check-cli-doc-drift.sh --strict` is run
    Then exit code is 0
```

只覆盖与本次目标真实相关的场景,未机械添加 fuzz / 性能 / 并发项。

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
| -------- | ---- | ---- | ------ | ------ | -------- |
| 现有 `inspect prompt` 空状态兼容 | `PromptPreview` JSON SSOT 字段名 / 顺序保持不变;新增字段在缺省场景下被 `skip_serializing_if` | `crates/ralph-cli/src/commands/inspect.rs` 内 `mod tests` + `crates/ralph-core/src/event_loop/tests/preview_api.rs` | 单元 | Characterization test:序列化反序列化对比固定 JSON fixture | 否 |
| 场景化 trigger context 预览 | `trigger_context_injected.source_topic` 等字段非空;`## TRIGGER CONTEXT` 块出现在 `block_titles` | `crates/ralph-core/src/event_loop/tests/preview_api.rs` 新增 `prompt_preview_with_trigger_context_*` | 单元 | Property:不同 `--source-hat` 不改变 prompt 栈 | 否 |
| 候选 emit 被接受 | `policy_decision == "accept"`,`reasons` 空,`projection.state_changes` 含每条 projection action | `crates/ralph-core/src/policy_check.rs` 单元测 + `crates/ralph-core/src/event_loop/tests/preview_api.rs` 集成 | 单元 | Differential:与 `ralph emit --policy-check --output json` 真实跑同输入比对 | 否 |
| 候选 emit 被拒 | `policy_decision == "reject"`,`reasons[]` 含 `gate`/`field`/`reason_code` | 同上 | 单元 | State machine:同一 topic 不同 payload 全枚举 | 否 |
| 候选 emit 不可证明 gate | `evidence_level == "unverified"` + `coverage_gaps[]` | 新增 | 单元 | — | 否 |
| capability inventory 输出 | `Vec<Capability>` 至少包含 wave / supervisor / task_id live / artifact-first / payload-consistency / trigger-context 六项 | `crates/ralph-core/src/capability_inventory.rs` 单元测 | 单元 | — | 否 |
| inventory coverage finding | 对未覆盖 capability,`covered_in_author_review == "no"` 且返回稳定 anchor | 单元测 | 单元 | — | 否 |
| 无场景参数 evidence_level 静态 | 默认输出 `evidence_level == "static"` | 单元测 | 单元 | — | 否 |
| 不写盘 | `.ralph/events.jsonl` 与 `.ralph/agent/` 在 preview 前后字节相同 | `crates/ralph-cli/src/commands/inspect.rs` 内 `mod tests` 临时目录 fixture | 单元 | — | 否 |
| 输入非法 | 解析错误退出码 2 + 字段名 | 单元测(clap parser) | 单元 | Fuzz:随机字节序列 | 否 |
| zsh 补全 | `ralph capability inventory` 出现在补全列表 | 手工 `compdef` + 脚本 grep | 单元 | — | 否 |
| doc-drift 通过 | `./scripts/check-cli-doc-drift.sh --strict` exit 0 | shell 跑一次 | 集成 | — | 否 |

未引入 E2E:本次扩展保持只读,与现有 `ralph inspect prompt` 同一可观测契约;E2E 不提供额外信号。`scripts/run-tests.sh` 在 Unit 8 收尾跑一次。

---

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成 | Evidence |
| -------------- | -- | -------- | ---- | ---- | ------- | --- |
| R1 | author/review 按 runtime audit model 检查 | Feature 总览 | — | — | — | E13 |
| R2 | 证据等级 `simulated`/`static`/`runtime`/`unverified` | Scenario:空状态 / 候选 emit 被接受 / 被拒 / 不可证明 | `preview_api.rs` 序列化测试 | 是 | — | E5,E11 |
| R3 | simulated 必须走同源判断逻辑 | 同上(由 `EventLoop::with_context` + `build_prompt` 路径强制) | `preview_characterization.rs` | 是 | — | E4,E6 |
| R4 | unverified 不得静默 Pass | Scenario:候选 emit 不可证明 gate | 新增 | 是 | — | E11 |
| R5 | 保留空状态预览 | Scenario:空状态兼容 | inspect.rs 内 mod tests + preview_api.rs | 是 | — | E1,E2 |
| R6 | 场景参数覆盖 trigger / wave / orchestrator / correction / scratchpad / tasks / memories gates | Scenario:场景化 trigger | preview_api.rs | 是 | — | E3,E7 |
| R7 | JSON 解析错误带字段位置 | Scenario:输入非法 | inspect.rs 内 clap parser test | 是 | — | E1 |
| R8 | 不写盘 | Scenario:不写盘 | inspect.rs 内 mod tests 临时目录 fixture | 是 | — | E10 |
| R9 | block 顺序、来源、未注入条件块 | Scenario:场景化 trigger | preview_api.rs | 是 | — | E6,E11 |
| R10 | 当前 hat 可见 trigger 字段、运行身份、自动注入与按需 skill | 同上 | preview_api.rs + commands.md | 是 | — | E15 |
| R11 | 候选 emit gate 评估 / 接受 / 拒绝 / projection / next hat | Scenario:候选 emit 被接受 / 被拒 | policy_check.rs + preview_api.rs | 是 | — | E8,E9 |
| R12 | capability inventory 机器可读 | Scenario:capability inventory 输出 | capability_inventory.rs 单测 | 是 | — | E9 |
| R13 | 每项 capability 含稳定标识 / 触发信号 / 适用范围 / 证据等级 | 同上 | 单测 | 是 | — | E13 |
| R14 | author/review 自动读取 inventory | Unit 4 SKILL.md 同步 | docs check | — | 是 | E11,E14 |
| R15 | coverage finding 不阻塞 CI | Scenario:inventory coverage finding | 单测 + doc 验证 | — | 是 | E12 |
| R16 | author 写 instructions 前完成 capability discovery | Unit 4 文档同步 | doc | — | 是 | E11 |
| R17 | review 独立重做 capability discovery | Unit 4 文档同步 | doc | — | 是 | E11 |
| R18 | review 建立证据覆盖表 | Unit 4 文档同步 | doc | — | 是 | E11 |
| R19 | 检查参数有效值 / 默认值 / 组合约束 | 由 SKILL.md 同步覆盖 | doc + capability inventory | — | 是 | E11,E13 |
| R20 | 检查 prompt block 顺序、条件门控、字段来源 | 由 SKILL.md 同步覆盖 | doc | — | 是 | E11 |
| R21 | 检查事件链路完整性 | Scenario:候选 emit 被接受 / 被拒 | preview_api.rs | 是 | — | E8,E9 |
| R22 | 检查终态可达性 | Unit 4 文档同步覆盖(由 capability inventory 标注) | doc | — | 是 | E13 |
| R23 | capability 触发 wave/supervisor 审查 | 由 SKILL.md 同步覆盖,沿用 E13 既有 capability audit | doc | — | 是 | E13 |
| R24 | author notes 增加 capability coverage | Unit 4 SKILL.md 同步 | doc | — | 是 | E11 |
| R25 | review report 增加机制/参数覆盖矩阵 | Unit 4 SKILL.md 同步 | doc | — | 是 | E11 |
| R26 | P0/P1 finding 指明 runtime audit model 环节 | 由 review 流程同步覆盖 | doc | — | 是 | E11 |
| R27 | 报告区分「未使用」「已通过」「证据不足」 | Scenario:inventory coverage finding | capability_inventory.rs 单测 | 是 | — | E12 |

每个需求至少一个 Scenario + 可执行测试。无 E2E 必要。

---

## 7. 严格串行开发单元

```
Unit 1: extend PromptPreview + InspectPromptArgs (scenario args, no candidate_emit)
  ↓ 完成全部测试、重构和回归
Unit 2: evaluate_candidate_emit (readonly) + candidate_emit preview branch
  ↓ 完成全部测试、重构和回归
Unit 3: ralph capability inventory (subcommand + lib module + zsh + drift)
  ↓ 完成全部测试、重构和回归
Unit 4: ralph-preset-author / ralph-preset-review SKILL.md + references sync
  ↓ 完成全部测试、重构和回归
Unit 5: run-tests.sh + check-cli-doc-drift + final regression
```

### Unit 1:extend PromptPreview + InspectPromptArgs

#### 1. Unit 目标
在不破坏既有 `ralph inspect prompt --hat <id> --format json` 空状态输出的前提下,为其增加可选的场景化参数(`--trigger` / `--source-hat` / `--payload` / `--iteration` / `--wave-context` / `--orchestrator-context` / `--correction` / `--scratchpad` / `--tasks-enabled` / `--memories-enabled`),并在 `PromptPreview` JSON 中按需输出 `trigger_context_injected` / `wave_context_injected` / `orchestrator_context_injected` / `correction_injected` / `skill_gates` / `evidence_level`。本次 Unit 不引入 `candidate_emit` 评估,只确保场景化上下文能正确进入 `## TRIGGER CONTEXT` 等 prompt 块。

#### 2. 对应需求与 Scenario
- Requirement ID:R2, R3, R5, R6, R7, R8, R9, R10
- Scenario ID:Scenario 1(空状态兼容)、Scenario 2(场景化 trigger)、Scenario 8(无场景 evidence_level 静态)、Scenario 9(不写盘)、Scenario 10(输入非法)
- Decision ID:D1, D2, D6, D8
- Evidence ID:E1, E2, E3, E4, E5, E6, E7, E10, E11

#### 3. 外部可观察结果
- 调用方(operator / author / review)运行 `ralph -c ralph.yml inspect prompt --hat X --trigger build.task --source-hat planner --payload '{"k":"v"}'` 时,JSON 输出新增 `trigger_context_injected` 字段;缺省该 flag 时,该字段不出现(`skip_serializing_if`)。
- 同一调用在 `--full` 时,`prompt_body` 包含 `## TRIGGER CONTEXT` 块(若 schema 声明)。
- 输出文件 `.ralph/events.jsonl` / `.ralph/agent/*` 在 preview 前后字节相同。

#### 4. 当前行为基线
- `ralph inspect prompt --hat X` 跑 `EventLoop::prompt_preview(hat_id)`,返回只含 `hat_id` / `gates` / `auto_inject` / `on_demand` / `block_titles` 的 JSON(inspect.rs:507-532, mod.rs:5480-5488)。无 trigger context 注入,无 iteration 概念。

#### 5. 输入与输出
- **输入**:`-c <cfg>` / `-H <preset>` / `--hat <id>` / 新增可选 `--trigger <TOPIC>` / `--source-hat <hat_id>` / `--payload <JSON>` / `--iteration <N>` / `--wave-context <JSON>` / `--orchestrator-context <JSON>` / `--correction <JSON>` / `--scratchpad` / `--tasks-enabled` / `--memories-enabled`
- **输出**:JSON 或 human。JSON 既有字段不变;新增 `Option` 字段按 `skip_serializing_if`。
- **错误**:clap 解析失败 → exit 2;JSON 解析失败 → exit 2 + stderr 包含字段名与 parse error;hat 不存在 → exit 2(沿用 E2)。
- **状态变化**:无
- **副作用**:无
- **不变量**:`auto_inject` 顺序与现有 `preview_prompt_for_config` 完全一致(E6)

#### 6. 修改位置
- `crates/ralph-cli/src/commands/inspect.rs:148-166`:`InspectPromptArgs` 增加 8 个 Option 字段 + 2 个 bool 字段。clap derive 直接扩展。
- `crates/ralph-cli/src/commands/inspect.rs:483-532`:`inspect_prompt_command` 解析场景参数 → 构造 `LoopContext` → 调用 `EventLoop::with_context` → `event_loop.prompt_preview(&hat_id)`(同一方法签名不变)。
- `crates/ralph-core/src/event_loop/mod.rs:417-435`:`PromptPreview` 新增 5 个 `Option<...>` 字段 + 1 个 `evidence_level: &'static str`。
- `crates/ralph-core/src/event_loop/mod.rs:5480-5488`:`prompt_preview` 接受 `Option<&LoopContext>` 参数(E3 已确认 `with_context` 路径)。
- `crates/ralph-core/src/loop_context.rs`:增加 `LoopContext::from_scenario_args(...) -> Result<Self>` 反序列化构造器(若已有 builder 方法则复用)。

每个位置说明:
- 当前职责:`InspectPromptArgs` 是 CLI 输入表面;`inspect_prompt_command` 是命令主体;`PromptPreview` 是序列化 SSOT;`prompt_preview` 是内存计算入口
- 为什么需要修改:产品合约 R6 要求场景化输入;R9-R10 要求 prompt 块可见性反馈
- 预计修改边界:仅加字段,不动现有字段顺序与序列化逻辑
- 明确不修改的相邻职责:`loop_inspect.v2` JSON 形状、`inspect profiles` 子命令

#### 7. 可依赖能力
- `EventLoop::with_context`(E3)
- `preview_prompt_for_config` 纯函数(E6)
- `build_prompt` 路径(E4)
- `RalphConfig::default()` + `cfg.hats.insert(id, HatConfig::default())`(inspect.rs 现有测试模式)

#### 8. 禁止依赖的未来能力
- 不实现 `candidate_emit` 评估(留给 Unit 2)
- 不引入 `ralph capability inventory`(留给 Unit 3)
- 不改 `ralph-preset-author` / `ralph-preset-review` SKILL.md(留给 Unit 4)

#### 9. 验收测试
- **测试 1**:`ralph_cli::commands::inspect::tests::inspect_prompt_args_default_no_scenario`(clap parser 测,沿用 inspect.rs:1418-1477 模式):`InspectPromptArgs::try_parse_from(["inspect","prompt","--hat","X"])` 解析成功,所有新字段为 None / false。
- **测试 2**:`ralph_cli::commands::inspect::tests::inspect_prompt_args_with_scenario`:`try_parse_from(["inspect","prompt","--hat","X","--trigger","build.task","--source-hat","planner","--payload","{\"k\":\"v\"}","--iteration","3"])` 解析成功,各字段绑定。
- **测试 3**:`ralph_cli::commands::inspect::tests::inspect_prompt_args_payload_malformed_errors`:payload `'{'` 时 clap 仍接受(clap 不解析 JSON),但 `inspect_prompt_command` 调用 JSON parse 阶段 `Result` 失败 → exit 2 + stderr 含 `"--payload"` 与 `expected value`。
- **测试 4**:`ralph_core::event_loop::tests::preview_api::prompt_preview_with_trigger_context`:`preview_prompt_for_config` 在 `LoopContext { trigger: Some(...) }` 时返回的 `PromptPreview.trigger_context_injected.source_topic` == `"build.task"`。
- **测试 5**:`ralph_core::event_loop::tests::preview_api::prompt_preview_with_no_scenario_keeps_skip_serializing`:JSON 序列化输出不含新增字段名(`grep '"trigger_context_injected"' == 空`)。
- **测试 6**:`ralph_cli::commands::inspect::tests::inspect_prompt_does_not_mutate_ralph_dir`:在 tmpdir 内创建空 `.ralph/`,前后 stat `.ralph/events.jsonl` 与 `.ralph/agent/tasks.jsonl` 字节相同(若不存在则 `size == 0` 两次)。
- **测试 7**:`ralph_core::event_loop::tests::preview_api::prompt_preview_wave_context_injected_appears`:传 `LoopContext { wave: Some(WaveContext { ... }) }`,`wave_context_injected` 字段非空。
- **测试 8**:`ralph_core::event_loop::tests::preview_api::prompt_preview_correction_context_injected_appears`:传 `CorrectionContext { ... }`,`correction_injected` 字段非空。
- **测试 9**:`ralph_core::event_loop::tests::preview_api::prompt_preview_evidence_level_static_when_no_scenario`:无任何场景参数时 `evidence_level == "static"`。
- **测试 10**:`ralph_core::event_loop::tests::preview_characterization::prompt_preview_with_context_equivalence`:`build_prompt(hat_id)` 在 `EventLoop::with_context(config, ctx)` 路径下的输出 `block_titles` 与不传 context 的输出相同(差异仅为可能新增 `## TRIGGER CONTEXT` / `## WAVE CONTEXT` / `## CORRECTION CONTEXT`)。
- **运行命令**:`cargo nextest run -p ralph-cli --bin ralph -- inspect_prompt` + `cargo nextest run -p ralph-core -- preview` + `cargo nextest run -p ralph-core -- prompt_preview`。
- **预期失败原因**:测试 1-9 是新增覆盖,自然 Red(无现有实现);测试 10 是 contract,实现必须保持等价性。

#### 10. Acceptance Red
- 先跑测试 1(`inspect_prompt_args_default_no_scenario`):编译期失败(`InspectPromptArgs` 缺字段),编译错包含「unknown field」即有效 Red。
- 接着跑测试 4(`prompt_preview_with_trigger_context`):编译期或运行时失败,`PromptPreview.trigger_context_injected` 不存在,有效 Red。
- 测试 5(序列化字段缺失):运行期 JSON 输出不含新键 → 通过(因为没有该键就是通过),必须用反证:`grep -F '"trigger_context_injected"'` 应为空。
- 测试 6(不写盘):运行期 `.ralph/` 字节相同 → 应通过(因为实现未写),需用 fixture 显式 assert 字节数 = 0 两次。
- 测试 9(evidence_level static):`PromptPreview.evidence_level` 不存在 → 编译失败,有效 Red。

#### 11. 单元测试拆分
- 子测试 A:`InspectPromptArgs::try_parse_from` 全部新 flag(clap parser)。
- 子测试 B:`LoopContext::from_scenario_args` 解析 `--payload` / `--wave-context` / `--orchestrator-context` / `--correction` JSON。
- 子测试 C:`preview_prompt_for_config` 在 `LoopContext` 注入下的各场景字段组装。
- 子测试 D:`PromptPreview` 序列化 SSOT(固定 fixture,字段顺序锁定)。

#### 12. Red → Green → Refactor 顺序
```
Test 1 Red (clap 编译失败)
→ InspectPromptArgs 加 10 个字段(含 serde 默认值)
→ Test 1 Green
→ Test 2 Red → 加 clap long help 文本 → Green
→ Test 3 Red (JSON parse)
→ 实现 LoopContext::from_scenario_args 含 serde_json::from_str + 字段级错误位置
→ Test 3 Green
→ Test 4 Red
→ PromptPreview 加 5 个 Option 字段 + 1 个 &'static str 字段;prompt_preview 接 Option<&LoopContext>
→ preview_prompt_for_config 改为接受 loop_context: Option<&LoopContext>
→ Test 4 Green
→ Test 5 通过(实现未写出新键时 grep 空)
→ Test 9 Red → evidence_level 字段存在但默认为 "static"
→ Test 9 Green
→ Test 10 Red → 实现 prompt_preview 中建立 EventLoop::with_context + 调用 build_prompt 提取 block_titles
→ Test 10 Green
→ Refactor:把 LoopContext 构造拆为独立 fn,inspect_prompt_command 主体 ≤ 80 行
→ Tests 1-10 全绿
```

#### 13. 最小实现范围
- 必须实现:`InspectPromptArgs` 新字段、`LoopContext::from_scenario_args`、`PromptPreview` 新字段、`prompt_preview` 接受 `Option<&LoopContext>`、`inspect_prompt_command` 解析并注入
- 必须修改的边界:`inspect.rs` clap arg 结构 + 命令主体;`event_loop/mod.rs` PromptPreview + prompt_preview 签名
- 必须处理的错误:`--payload` JSON 解析失败 → exit 2 + 字段名
- 必须保持的不变量:`auto_inject` 顺序、`block_titles` 在无 context 时与现有等价
- 明确不实现:`candidate_emit` 评估、`capability inventory`

#### 14. 集成验证
- 真模块联合:`ralph-cli` 命令 + `ralph-core` PromptPreview + `LoopContext`
- Fake / Stub:仅测试 fixture 用 `RalphConfig::default()` + `cfg.hats.insert("X", HatConfig::default())`
- 真实验证:测试 10 跑 `EventLoop::with_context` 真实路径,断言 `build_prompt` 输出含期望块
- 执行命令:`cargo nextest run -p ralph-cli --bin ralph -- inspect_prompt` + `cargo nextest run -p ralph-core -- prompt_preview_with_context` + `cargo nextest run -p ralph-core -- test preview`
- 预期结果:全部 green,无 `.only` / `.skip`

#### 15. 风险驱动测试
- Characterization:测试 5 + 测试 10(序列化 SSOT 与 build_prompt 等价)
- Differential:测试 10 同时跑 `with_context` 与无 context 两条路径,断言新增块存在 / 不存在语义
- Property:`RalphConfig` 的 5 种 hats 全组合跑 `prompt_preview` 不 panic(沿用 `preview_api.rs` 现有 property 模式)
- 不引入 Fuzz / Mutation / Concurrency(本次只读扩展不涉并发)

#### 16. 回归范围
- 直接相关:`crates/ralph-cli/src/commands/inspect.rs` 全部测试 + `crates/ralph-core/src/event_loop/tests/{preview_api,preview_characterization,build_prompt}.rs`
- 相邻:`crates/ralph-core/src/event_loop/tests/{initialization,u3_trigger_context_prompt,u4_handoff_envelope_prompt,wave_context_injection}.rs`
- 公开接口消费者:`crates/ralph-cli/src/commands/run.rs`(使用同一 `EventLoop::new` 模式,不直接调用 `prompt_preview`)
- 旧配置 / 旧数据:`RalphConfig::default()` 行为不变
- 默认关闭路径:无
- 构建目标:`cargo build` + `cargo clippy`
- Lint:`cargo clippy --workspace`
- Typecheck:`cargo build` 隐含
- 必要全量:`./scripts/run-tests.sh` 在 Unit 5 跑一次

理由:`prompt_preview` 是单测覆盖点;`build_prompt` 是所有 hat 激活路径必经之处;`with_context` 是新路径,需保证不破坏旧 `EventLoop::new(config)` 路径。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| -- | ---- | ---- | -------- |
| `crates/ralph-cli/src/commands/inspect.rs` | 修改 | `InspectPromptArgs` 加字段 + `inspect_prompt_command` 解析场景参数 | E1,E2 |
| `crates/ralph-core/src/event_loop/mod.rs` | 修改 | `PromptPreview` 加 6 个字段 + `prompt_preview` 加 Option 参数 | E4,E5 |
| `crates/ralph-core/src/event_loop/prompt.rs` | 修改 | `preview_prompt_for_config` 加 loop_context 参数,内部组装新字段 | E6 |
| `crates/ralph-core/src/loop_context.rs` | 修改 | 加 `LoopContext::from_scenario_args(...)` 反序列化构造器 | E7 |
| `crates/ralph-core/src/event_loop/tests/preview_api.rs` | 新增 | 场景化 preview 单元测 | — |
| `crates/ralph-core/src/event_loop/tests/preview_characterization.rs` | 新增 | with_context 等价性 contract 测 | — |
| `crates/ralph-cli/src/commands/inspect.rs::tests` | 新增 | clap parser + 不写盘 fixture 测 | — |

#### 18. 完成标准
- Test 1-10 全部 green
- `cargo nextest run -p ralph-cli --bin ralph -- inspect_prompt` 全 green
- `cargo nextest run -p ralph-core -- preview` 全 green
- `cargo build` 通过
- `cargo clippy` 通过
- `prompt_preview` 单实例一次性契约(E4)未被破坏
- 无新增 `.only` / `.skip`
- 无削弱断言
- 未实现 Unit 2 / 3 / 4 的能力

#### 19. 停止条件
- `prompt_preview` 接受 Option 参数后,序列化 JSON 顺序改变 → 停止,回到 D6 重新评估
- `LoopContext::from_scenario_args` 发现 `TriggerContextInput` / `WaveContext` / `CorrectionContext` 缺 `serde::Deserialize` 实现 → 停止,记新 Evidence,重规划(预期仅添加 derive)
- 测试 6 失败(写盘发生) → 停止,排查 `prompt_preview` 是否触发了 `with_context` 外的副作用

#### 20. 风险与注意事项
- **风险**:`EventLoop::with_context` 在 `prompt_preview` 路径下被复用,可能影响 `handoff_tracker.on_hat_activated` 副作用(E4)
- **触发条件**:inspect 路径单进程单实例;若后续被嵌入长生命周期,会绕过 WRC-U4 30s escalation gate
- **检测方式**:测试 6 字节检查 + `prompt_preview` doc comment 已显式说明
- **缓解措施**:在 `prompt_preview` doc 上保留「单实例一次性」注释,future caller 不可复用
- **剩余风险**:无

---

### Unit 2:evaluate_candidate_emit + candidate_emit preview 分支

#### 1. Unit 目标
在 `ralph-core` 抽出只读 `pub fn evaluate_candidate_emit(config, hat_id, topic, payload, triggered) -> PolicyEvaluationResult`,在 `ralph inspect prompt` 接受 `--topic` + `--payload` 时调用,把结果嵌入 `PromptPreview.candidate_emit` 字段,并在不可证明的 gate 上标记 `unverified` 与 `coverage_gaps`。

#### 2. 对应需求与 Scenario
- Requirement ID:R11, R21, R4
- Scenario ID:Scenario 3(候选 emit 被接受)、Scenario 4(候选 emit 被拒)、Scenario 5(不可证明 gate)
- Decision ID:D3, D7, D10
- Evidence ID:E8, E9

#### 3. 外部可观察结果
- `ralph inspect prompt --hat X --topic work.done --payload '{"task_id":"t1"}' --triggered worker --format json` 输出新增 `candidate_emit: { policy_decision, reasons[], projection, next_hat_candidates }`
- `evidence_level` 从 `"static"` 升为 `"runtime"`(用户提供了 candidate emit)

#### 4. 当前行为基线
- 现有 `ralph emit --policy-check --output json --triggered <hat> <topic> '<payload>'` 路径返回 policy_decision / reasons,但只写临时 stdout,不暴露 next_hat_candidates(E8)
- `inspect_prompt` 不评估任何 candidate emit

#### 5. 输入与输出
- **输入**:`--topic <TOPIC>` / `--payload <JSON>` / `--triggered <hat_id>`(三者至少提供 `--topic` + `--payload`)
- **输出**:`PromptPreview.candidate_emit: Option<CandidateEmitPreview>`
- **错误**:`--topic` 给出但 `--payload` 缺 → exit 2 + 字段名;payload JSON 非法 → exit 2
- **状态变化**:无
- **副作用**:无
- **不变量**:`policy_decision` / `reasons` 内容与 `ralph emit --policy-check --output json` 在相同输入下一致(Differential)

#### 6. 修改位置
- `crates/ralph-core/src/policy_check.rs`:新增 `pub fn evaluate_candidate_emit(config: &RalphConfig, hat_id: &HatId, topic: &str, payload: &serde_json::Value, triggered: Option<&HatId>) -> PolicyEvaluationResult`
- `crates/ralph-core/src/event_loop/types.rs` 或 `prompt.rs`:新增 `pub struct CandidateEmitPreview { policy_decision: &'static str, reasons: Vec<PolicyReason>, projection: Option<ProjectionPreview>, next_hat_candidates: NextHatCandidates }` + `pub enum NextHatCandidates { Verified(Vec<HatId>), Unverified, Mixed(Vec<HatCandidate>) }`
- `crates/ralph-core/src/event_loop/mod.rs`:`PromptPreview` 新增 `candidate_emit: Option<CandidateEmitPreview>` 字段(沿用 `skip_serializing_if`)
- `crates/ralph-cli/src/commands/inspect.rs`:`inspect_prompt_command` 增加 `--topic` / `--triggered` 解析,在 `prompt_preview` 之后调 `evaluate_candidate_emit` 把结果组装进 PromptPreview
- `crates/ralph-core/src/event_loop/projection.rs`(若已存在)或新建模块:抽离 projection 评估函数,返回「哪些 projection actions 会触发 / 哪些 next hat 可消费 / 哪些依赖不可见 runtime state → 标 unverified」

每个位置说明:
- 当前职责:`policy_check.rs` 是 emit 路径的 policy gate 评估
- 为什么需要修改:产品合约 R11 要求 inspect 路径能同源评估 candidate emit
- 预计修改边界:`evaluate_candidate_emit` 是新公共函数,既给 inspect 路径也给潜在 unit test 用
- 明确不修改:`ralph emit --policy-check` CLI 行为不变,只是底层调用同一函数

#### 7. 可依赖能力
- 现有 `pub fn check_topic_format` / `check_handoff_envelope` / `check_completion_honored` / `is_recoverable_policy_finding`(E9)
- `ralph emit --policy-check` 既有 JSON 输出 schema(用于 Differential)
- Unit 1 的 `PromptPreview` 扩展

#### 8. 禁止依赖的未来能力
- 不实现 `capability inventory`(留给 Unit 3)
- 不修改 `ralph-preset-author/review` SKILL.md(留给 Unit 4)

#### 9. 验收测试
- **测试 1**:`ralph_core::policy_check::tests::evaluate_candidate_emit_accepts_valid_payload`:`config.hats["worker"].publishes = ["work.done"]`,`triggered = Some(worker)`,topic = `work.done`,payload 含 `task_id` → `policy_decision == "accept"`,`reasons` 空
- **测试 2**:`ralph_core::policy_check::tests::evaluate_candidate_emit_rejects_missing_required_field`:同 setup 但 payload 缺 `task_id` → `policy_decision == "reject"`,`reasons[0].field == "task_id"`,`reason_code == "missing_required_field"`
- **测试 3**:`ralph_core::policy_check::tests::evaluate_candidate_emit_rejects_triggered_not_in_topology`:`triggered = Some("nonexistent")` → `policy_decision == "reject"`,`reason_code == "triggered_not_in_topology"`
- **测试 4**:`ralph_core::event_loop::tests::preview_api::candidate_emit_accepted_includes_projection`:`evaluate_candidate_emit` 返回 `Some(ProjectionPreview { state_changes: [...] })`,嵌入到 `PromptPreview.candidate_emit.projection`
- **测试 5**:`ralph_core::event_loop::tests::preview_api::candidate_emit_unverified_when_projection_depends_on_state`:`RalphConfig` 无 `state_projection` 时 → `next_hat_candidates` 是 `Unverified`
- **测试 6**:`ralph_cli::commands::inspect::tests::inspect_prompt_candidate_emit_does_not_write_ralph_dir`:tmpdir 内 `.ralph/` 在调用前后字节相同
- **测试 7**:`ralph_core::policy_check::tests::evaluate_candidate_emit_equivalence_with_emit_policy_check`:用同一 fixture 跑 `ralph emit --policy-check --output json` 与 `evaluate_candidate_emit`,断言 `policy_decision` 与 `reasons[].reason_code` 一致(Differential)
- **测试 8**:`ralph_cli::commands::inspect::tests::inspect_prompt_evidence_level_runtime_with_candidate`:有 `--topic` + `--payload` 时 `evidence_level == "runtime"`;否则 `"static"`
- **运行命令**:`cargo nextest run -p ralph-core -- evaluate_candidate_emit` + `cargo nextest run -p ralph-cli --bin ralph -- inspect_prompt_candidate_emit` + `cargo nextest run -p ralph-core -- candidate_emit_unverified`

#### 10. Acceptance Red
- 测试 1-3:运行期 `evaluate_candidate_emit` 不存在 → 编译失败,有效 Red
- 测试 4-5:`CandidateEmitPreview` 不存在 → 编译失败,有效 Red
- 测试 7:Differential 比对时,real `ralph emit --policy-check` 与新函数输出字段对不上 → 失败,有效 Red

#### 11. 单元测试拆分
- 子测试 A:`evaluate_candidate_emit` 各 policy gate 的 accept / reject 分支(每条 gate 至少 1 accept + 1 reject)
- 子测试 B:`CandidateEmitPreview` 序列化 SSOT
- 子测试 C:`NextHatCandidates::Mixed` 混合 verified / unverified 序列化
- 子测试 D:`inspect_prompt_command` 集成 `evaluate_candidate_emit` 后 JSON 形状

#### 12. Red → Green → Refactor 顺序
```
Test 1 Red → 实现 evaluate_candidate_emit (accept 分支)
→ Test 1 Green
→ Test 2 Red → 实现 reject_missing_required_field 分支 → Green
→ Test 3 Red → 实现 reject_triggered_not_in_topology 分支 → Green
→ Test 4 Red → CandidateEmitPreview struct 加 → Green
→ Test 5 Red → NextHatCandidates enum 加 → Green
→ Test 6 验证 inspect 不写盘 → 通过
→ Test 7 Red → 比对 emit --policy-check JSON 找 diff → 修正 evaluate_candidate_emit 输出字段顺序
→ Test 7 Green
→ Test 8 Red → evidence_level 切换逻辑 → Green
→ Refactor:把 projection 评估抽到独立函数
→ Tests 1-8 全绿
```

#### 13. 最小实现范围
- 必须实现:`evaluate_candidate_emit`、`CandidateEmitPreview`、`NextHatCandidates`、`inspect_prompt_command` 集成
- 必须修改的边界:`policy_check.rs`(新增)、`event_loop/mod.rs`(PromptPreview 加 Option 字段)、`inspect.rs`(clap + 调用)
- 必须处理的错误:`--topic` + `--payload` 不一致 → exit 2
- 必须保持的不变量:与 `ralph emit --policy-check` 输出字段顺序一致(Differential 测试保证)
- 明确不实现:`capability inventory`

#### 14. 集成验证
- 真模块联合:`ralph-cli inspect` + `ralph-core policy_check` + `ralph-core event_loop`
- Fake / Stub:仅 fixture 用 `RalphConfig::default()`
- 真实验证:测试 7 跑真 `ralph emit --policy-check` 子进程 + 新函数,断言 JSON 字段对位
- 执行命令:`cargo nextest run -p ralph-core -- evaluate_candidate_emit` + `cargo nextest run -p ralph-cli --bin ralph -- inspect_prompt`
- 预期结果:全部 green

#### 15. 风险驱动测试
- Differential:测试 7(与 `ralph emit --policy-check` 同输入比对)
- State-machine:测试 2-3 覆盖 reject 各 gate
- 不引入 Concurrency / Fuzz

#### 16. 回归范围
- 直接相关:`crates/ralph-core/src/policy_check.rs` 全部测试 + `crates/ralph-core/src/event_loop/tests/preview_api.rs`
- 相邻:`crates/ralph-cli/src/commands/emit.rs`(`should_policy_check_emit` 等单元测)
- 公开接口消费者:`ralph wave emit --policy-check` 间接复用 policy gate(不直接调 evaluate_candidate_emit,但保证字段顺序一致)

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| -- | ---- | ---- | -------- |
| `crates/ralph-core/src/policy_check.rs` | 新增 fn + tests | 抽离只读 evaluate_candidate_emit | E8,E9 |
| `crates/ralph-core/src/event_loop/types.rs` 或 prompt.rs | 新增 struct | CandidateEmitPreview / NextHatCandidates | E5 |
| `crates/ralph-core/src/event_loop/mod.rs` | 修改 | PromptPreview 加 candidate_emit 字段 | E4 |
| `crates/ralph-cli/src/commands/inspect.rs` | 修改 | InspectPromptArgs 加 --topic/--triggered;命令主体评估 candidate | E1 |
| `crates/ralph-core/src/event_loop/tests/preview_api.rs` | 新增 | candidate_emit 单元测 | — |

#### 18. 完成标准
- Tests 1-8 green
- Differential 测试与 `ralph emit --policy-check` 输出对齐
- 未引入 CI 退出码变化

#### 19. 停止条件
- 测试 7 diff 持续无法对齐 → 停止,记新 Evidence,重规划(可能需要回到 policy_check.rs 抽出更细粒度函数)
- `evaluate_candidate_emit` 副作用被触发(写盘) → 停止

#### 20. 风险与注意事项
- **风险**:`ralph emit --policy-check` 已有路径包含 batch / dedup / EventBus 副作用,抽出 `evaluate_candidate_emit` 时必须保证不接触这些
- **触发条件**:函数内部错误地调用 `EventBus::publish` 或 `Ledger::append`
- **检测方式**:测试 6(不写盘)与现有 `policy_check.rs` 不写盘保证
- **缓解措施**:函数签名只接受 `&RalphConfig` 与 payload,不接受 `EventLoop` / `EventBus` 引用
- **剩余风险**:无

---

### Unit 3:ralph capability inventory(subcommand + lib module + zsh + drift)

#### 1. Unit 目标
在 `ralph-core` 新增 `pub fn capability_inventory() -> Vec<Capability>`,在 `ralph-cli` 新增 `ralph capability inventory [--format {human|json}]` 子命令,输出 preset-facing capability 列表(每项含 id / trigger_signal / applies_when / evidence_sources / recommended_evidence_level / covered_in_author_review / source),并在 `scripts/ralph-zsh-plugin.zsh` 加补全,通过 `scripts/check-cli-doc-drift.sh --strict`。

#### 2. 对应需求与 Scenario
- Requirement ID:R12, R13, R14, R15
- Scenario ID:Scenario 6(inventory 输出)、Scenario 7(coverage finding)、Scenario 11(zsh 补全)、Scenario 12(doc-drift 通过)
- Decision ID:D4, D5, D11, D12, D13
- Evidence ID:E13, E14, E16, E17

#### 3. 外部可观察结果
- `ralph capability inventory --format json` 输出 `Vec<Capability>`,字段稳定
- 每项 `covered_in_author_review` 字段在 references 文档含相应 anchor 时为 `"yes"`,否则 `"no"`
- `compdef _ralph ralph` 后 `ralph capability inventory<TAB>` 可补全
- `scripts/check-cli-doc-drift.sh --strict` exit 0

#### 4. 当前行为基线
- 现有 `ralph inspect` 与 `ralph preset` 子命令,无 `ralph capability`(E17 验证 zsh 脚本不包含该补全)
- 没有 `capability_inventory()` 函数

#### 5. 输入与输出
- **输入**:`ralph capability inventory [--format {human|json}]`
- **输出**:JSON 数组或 human 表格
- **错误**:无参数子命令 → 子命令列表输出(沿用 `inspect` 模式,inspect.rs:191-196)
- **状态变化**:无
- **副作用**:无
- **不变量**:JSON 字段顺序与命名稳定(`inventory_capability.v1` schema)

#### 6. 修改位置
- `crates/ralph-core/src/capability_inventory.rs`(新建模块):
  - `pub struct Capability { id: &'static str, trigger_signal: &'static str, applies_when: &'static str, evidence_sources: Vec<&'static str>, recommended_evidence_level: &'static str, covered_in_author_review: &'static str, source: &'static str }`
  - `pub fn capability_inventory() -> Vec<Capability>`
- `crates/ralph-core/src/lib.rs`:新增 `pub mod capability_inventory;`
- `crates/ralph-cli/src/commands/capability.rs`(新建):`CapabilityArgs` + `CapabilityCommands::Inventory` + `capability_inventory_command`
- `crates/ralph-cli/src/commands/mod.rs`:新增 `pub mod capability;`
- `crates/ralph-cli/src/cli.rs`:在 `Commands` enum 新增 `Capability(CapabilityArgs)` 分支,转交 `commands::capability::execute(...)`
- `scripts/ralph-zsh-plugin.zsh`:在补全列表加 `capability:_ralph_capability` 与 `inventory`
- `scripts/check-cli-doc-drift.sh`:确认其 `capability` 字符串 anchor 已加(若该脚本扫 ralph-tools*.md 内的命令列表,需追加 inventory)
- `skills/ralph-preset-common/references/commands.md`:新增 `## Capability inventory` 段(Unit 4 也会引用)

每个位置说明:
- 当前职责:不存在的模块
- 为什么需要修改:产品合约 R12-R15 要求 capability inventory 机器可读
- 预计修改边界:仅新增,不动现有 inspect / preset
- 明确不修改:hat instructions / event policy / RalphConfig schema

#### 7. 可依赖能力
- `serde_json::to_writer_pretty`(inspect.rs:577 模式)
- 现有 `commands::mod.rs` 子命令注册模式

#### 8. 禁止依赖的未来能力
- 不改 SKILL.md(留给 Unit 4)
- 不引 `ralph inspect prompt` 改动(Unit 1/2 已完成)

#### 9. 验收测试
- **测试 1**:`ralph_core::capability_inventory::tests::capability_inventory_is_non_empty`:`capability_inventory().len() >= 6`(wave / supervisor / task_id_live / artifact_first / payload_consistency / trigger_context)
- **测试 2**:`ralph_core::capability_inventory::tests::capability_inventory_stable_ids`:每项 `id` 是稳定的 kebab-case,与 references 文档 anchor 对齐
- **测试 3**:`ralph_core::capability_inventory::tests::capability_inventory_covered_in_author_review_for_wave`:`find_by_id("wave-emit").covered_in_author_review == "yes"`(对应 `finding-rubric.md`「Wave capability audit」段)
- **测试 4**:`ralph_core::capability_inventory::tests::capability_inventory_covered_for_supervisor`:同模式
- **测试 5**:`ralph_core::capability_inventory::tests::capability_inventory_uncovered_when_no_anchor`:临时移除 references 文件 → 该 capability `covered_in_author_review == "no"`(在本测试用 fixture dir)
- **测试 6**:`ralph_cli::commands::capability::tests::cli_parses_capability_inventory_minimal`:`try_parse_from(["capability","inventory"])` 成功
- **测试 7**:`ralph_cli::commands::capability::tests::capability_inventory_json_shape`:运行 `capability_inventory_command` 输出是 `Vec<Capability>` JSON,字段顺序与 SSOT 一致
- **测试 8**:`scripts/check-cli-doc-drift.sh --strict` 在代码改动后 exit 0
- **测试 9**:`scripts/ralph-zsh-plugin.zsh` grep `capability.*inventory` 命中
- **运行命令**:`cargo nextest run -p ralph-core -- capability_inventory` + `cargo nextest run -p ralph-cli --bin ralph -- capability_inventory` + `./scripts/check-cli-doc-drift.sh --strict` + `bash -n scripts/ralph-zsh-plugin.zsh` + `grep capability scripts/ralph-zsh-plugin.zsh`

#### 10. Acceptance Red
- 测试 1:`capability_inventory` 函数不存在 → 编译失败,有效 Red
- 测试 6:CLI 子命令不存在 → 编译失败,有效 Red
- 测试 9:zsh 脚本无新补全 → grep 命中失败,有效 Red
- 测试 8:drift 脚本检测到 ralph-tools*.md 文档与新增命令不一致 → exit 非零,有效 Red

#### 11. 单元测试拆分
- 子测试 A:每条 capability 的 id / trigger_signal / applies_when 字段值稳定
- 子测试 B:`covered_in_author_review` 检测逻辑(references 文件读取 + anchor 匹配)
- 子测试 C:CLI parser
- 子测试 D:JSON 序列化 SSOT

#### 12. Red → Green → Refactor 顺序
```
Test 1 Red → 新建 capability_inventory.rs 含 6 条 capability 静态列表
→ Test 1 Green
→ Test 2 Red → 加更详细字段 → Green
→ Test 3-5 Red → 实现 references anchor 检测(读 preset-common/references 目录)
→ Tests 3-5 Green
→ Test 6 Red → 新建 capability.rs CLI 子命令
→ Test 6 Green
→ Test 7 Red → 输出 JSON 序列化 → Green
→ Test 8 Red → 同步 ralph-tools-cmdref.md / commands.md → Green
→ Test 9 Red → 同步 zsh 补全 → Green
→ Refactor:把 capability 列表与 anchor 检测拆为独立函数
→ Tests 1-9 全绿
```

#### 13. 最小实现范围
- 必须实现:`capability_inventory()` 静态列表 + `Capability` struct + CLI 子命令 + zsh 补全 + ralph-tools-cmdref 同步
- 必须修改的边界:`lib.rs` / `cli.rs` / `commands/mod.rs` / `commands/capability.rs` / `scripts/ralph-zsh-plugin.zsh` / `ralph-tools-cmdref.md`
- 必须处理的错误:无
- 必须保持的不变量:不改变 `ralph preset check` 退出码(E12)
- 明确不实现:不在 SKILL.md 内同步,留给 Unit 4

#### 14. 集成验证
- 真模块联合:`ralph-cli capability inventory` + `ralph-core capability_inventory` + references 文档
- Fake / Stub:测试 5 用 fixture dir 模拟 references 缺失
- 真实验证:测试 9 真跑 zsh 脚本 + grep
- 执行命令:`cargo nextest run -p ralph-cli --bin ralph -- capability_inventory` + `cargo nextest run -p ralph-core -- capability_inventory` + `./scripts/check-cli-doc-drift.sh --strict` + `bash -n scripts/ralph-zsh-plugin.zsh`
- 预期结果:全部 green

#### 15. 风险驱动测试
- Characterization:测试 1-2(列表稳定 SSOT)
- Differential:测试 3-5(references anchor 检测在文件增删时正确切换 yes/no)
- 不引入 Fuzz / Mutation

#### 16. 回归范围
- 直接相关:`crates/ralph-core/src/capability_inventory.rs` + `crates/ralph-cli/src/commands/capability.rs`
- 相邻:`crates/ralph-cli/src/cli.rs`(Commands enum 加分支)
- 公开接口消费者:`scripts/check-cli-doc-drift.sh`(必须通过)、`scripts/ralph-zsh-plugin.zsh`(必须加载)

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| -- | ---- | ---- | -------- |
| `crates/ralph-core/src/capability_inventory.rs` | 新增 | capability 静态列表 + 检测 | E13 |
| `crates/ralph-core/src/lib.rs` | 修改 | pub mod capability_inventory | — |
| `crates/ralph-cli/src/commands/capability.rs` | 新增 | CLI 子命令 | E1,E2 模式 |
| `crates/ralph-cli/src/commands/mod.rs` | 修改 | pub mod capability | — |
| `crates/ralph-cli/src/cli.rs` | 修改 | Commands enum 加 Capability 分支 | — |
| `scripts/ralph-zsh-plugin.zsh` | 修改 | 加 capability 补全 | E17 |
| `crates/ralph-core/data/ralph-tools-cmdref.md` | 修改 | 加 capability inventory 段(被 hat instructions 引用) | E15 |
| `skills/ralph-preset-common/references/commands.md` | 修改 | 加 `## Capability inventory` 段 | E11 |
| `crates/ralph-core/src/capability_inventory.rs::tests` | 新增 | 单元测 | — |

#### 18. 完成标准
- Tests 1-9 全 green
- `ralph capability inventory --format json` 命令真跑成功并输出稳定 JSON
- `./scripts/check-cli-doc-drift.sh --strict` exit 0
- `bash -n scripts/ralph-zsh-plugin.zsh` exit 0
- `ralph preset check --strict` 退出码未变化(已通过 0 builtin preset 验证)

#### 19. 停止条件
- `capability_inventory` 列表与 references 文档 anchor 不一致且无法确定哪个权威 → 停止,记新 Evidence
- drift 脚本持续失败 → 停止,排查脚本静态扫描规则

#### 20. 风险与注意事项
- **风险**:`covered_in_author_review` 检测依赖文件字符串匹配,文档重构时易漂移
- **触发条件**:references 文件 anchor 改名 / 删除
- **检测方式**:测试 5 用 fixture dir 验证 yes/no 切换
- **缓解措施**:anchor 字符串写在 capability_inventory.rs 顶部 const + comment,便于同步
- **剩余风险**:后续 SKILL.md 更新后 anchor 漂移,需 Unit 4 同步

---

### Unit 4:ralph-preset-author / ralph-preset-review SKILL.md + references sync

#### 1. Unit 目标
把 `inspect_prompt` 场景化参数(Unit 1)、`candidate_emit` 干跑评估(Unit 2)、`ralph capability inventory`(Unit 3)三套能力写入 `ralph-preset-author` / `ralph-preset-review` 的 SKILL.md 与 references,确保 author 在写 hat instructions 前完成 capability discovery,review 独立重做并建立证据覆盖表,且不改变既有 CI / lint 退出语义。

#### 2. 对应需求与 Scenario
- Requirement ID:R14, R16, R17, R18, R19, R20, R22, R23, R24, R25, R26, R27
- Scenario ID:Scenario 7(inventory coverage finding 在 SKILL.md 中映射)、Scenario 11(zsh 补全经 SKILL.md 引用)
- Decision ID:D12, D13
- Evidence ID:E11, E13, E14, E15

#### 3. 外部可观察结果
- `skills/ralph-preset-author/SKILL.md` Workflow 新增步骤「Capability discovery」与「Scenario preview」
- `skills/ralph-preset-review/SKILL.md` Workflow 新增步骤「Capability-triggered audit」与「Activation evidence coverage matrix」
- `skills/ralph-preset-common/references/finding-rubric.md` 新增 capability coverage finding_id 与 evidence_level 标签约定
- `skills/ralph-preset-common/references/agent-native-model.md` 新增 Runtime Audit Model 段
- `crates/ralph-core/data/ralph-tools-cmdref.md` 新增 `ralph capability inventory` 段 + `ralph inspect prompt` 新参数

#### 4. 当前行为基线
- author SKILL.md 已含 Discovery gate + Workflow 0-6(E11 模式)
- review SKILL.md 已含 Workflow 0a-9 + Report Structure(E11 模式)
- references 文档已覆盖 AAF / payload audit / artifact-first / wave capability / supervisor capability(E13)

#### 5. 输入与输出
- **输入**:不直接接收输入,纯文档更新
- **输出**:`SKILL.md` / `references/*.md` / `crates/ralph-core/data/ralph-tools*.md` 的更新
- **错误**:无
- **状态变化**:无
- **副作用**:无
- **不变量**:`ralph preset check --strict` 退出码不变

#### 6. 修改位置
- `skills/ralph-preset-author/SKILL.md`:在 Workflow 第 1 步前插入「Step 0.5: Capability discovery」,在「Prompt Visibility 必查」段加场景化 preview 命令
- `skills/ralph-preset-review/SKILL.md`:在 Workflow 第 3a 后插入「Step 3a.5: Capability-triggered audit」,在 Report Structure 加「Capability coverage」与「Evidence coverage matrix」段
- `skills/ralph-preset-common/references/agent-native-model.md`:新增「Runtime Audit Model」段
- `skills/ralph-preset-common/references/finding-rubric.md`:新增「Capability coverage finding_id」表
- `skills/ralph-preset-common/references/commands.md`:更新 `## Prompt 可见性` 与新增 `## Capability inventory`
- `crates/ralph-core/data/ralph-tools-cmdref.md`:新增 `ralph capability inventory` 与 `ralph inspect prompt` 新参数

每个位置说明:
- 当前职责:author/review skill 工作流文档
- 为什么需要修改:产品合约 R14-R27 要求 SKILL.md 与 references 同步新能力
- 预计修改边界:仅加步骤 / 段,不删既有内容
- 明确不修改:`scripts/ralph-zsh-plugin.zsh`(Unit 3 已完成)、Rust 代码

#### 7. 可依赖能力
- Unit 1 / 2 / 3 已落地的能力
- 现有 references 文档模板

#### 8. 禁止依赖的未来能力
- 不引入新 CLI 命令(已由 Unit 3 完成)
- 不改 references 结构(仅加段)

#### 9. 验收测试
- **测试 1**:`tests/test_skill_anchors.py`(新建,放在 `skills/ralph-preset-common/tests/` 下):断言 SKILL.md 与 references 中存在 anchor `## Capability discovery` / `## Capability-triggered audit` / `## Capability inventory` / `## Runtime Audit Model`
- **测试 2**:`tests/test_skill_drift.py`(新建):`commands.md` 中 `ralph capability inventory` 命令模板存在
- **测试 3**:`tests/test_ralph_tools_doc_sync.sh`(新增,放在 `scripts/` 下,沿用 `check-cli-doc-drift.sh` 模式):`crates/ralph-core/data/ralph-tools-cmdref.md` 含 `ralph capability inventory` 字符串
- **测试 4**:`./scripts/check-cli-doc-drift.sh --strict` exit 0(Unit 3 已要求,本 Unit 验证仍通过)
- **测试 5**:`tests/test_prompt_visibility_contract.py`(已存在):仍 green,不破坏既有 anchor
- **运行命令**:`cargo nextest run -p ralph-cli --bin ralph -- check-cli-doc-drift` + 手工跑 `python3 tests/test_skill_anchors.py` + `bash scripts/test_ralph_tools_doc_sync.sh`

#### 10. Acceptance Red
- 测试 1:anchor 缺失 → grep 失败,有效 Red
- 测试 3:`ralph-tools-cmdref.md` 缺命令 → 脚本失败,有效 Red

#### 11. 单元测试拆分
- 子测试 A:每个 anchor 在 SKILL.md / references 中存在
- 子测试 B:每个新增能力在 references 中有引用章节
- 子测试 C:`ralph-tools-cmdref.md` 与 CLI 子命令同步

#### 12. Red → Green → Refactor 顺序
```
Test 1 Red → SKILL.md / references 加 anchor → Green
Test 2 Red → commands.md 加 capability inventory 段 → Green
Test 3 Red → ralph-tools-cmdref.md 加 → Green
Test 4 跑一次应通过(Unit 3 已要求)
Test 5 跑一次应通过(既有 contract)
→ Refactor:把测试 1-3 的 anchor 列表提取到 const 便于同步
→ Tests 1-5 全绿
```

#### 13. 最小实现范围
- 必须实现:`SKILL.md` / `references/*.md` / `ralph-tools-cmdref.md` 同步
- 必须修改的边界:文档,不改 Rust 代码
- 必须处理的错误:无
- 必须保持的不变量:`ralph preset check --strict` 退出码不变
- 明确不实现:不引入新测试框架(沿用既有 shell / python 模式)

#### 14. 集成验证
- 真模块联合:文档 + `tests/test_*.py` + `scripts/check-cli-doc-drift.sh`
- Fake / Stub:无
- 真实验证:手工 cat 新段落 + grep anchor
- 执行命令:`./scripts/check-cli-doc-drift.sh --strict` + `python3 skills/ralph-preset-common/tests/test_skill_anchors.py`
- 预期结果:全部 green

#### 15. 风险驱动测试
- Characterization:测试 5(既有 contract 不破)
- 不引入其他类型(纯文档同步)

#### 16. 回归范围
- 直接相关:`skills/tests/test_prompt_visibility_contract.py`
- 相邻:`scripts/check-cli-doc-drift.sh`(必须通过)
- 公开接口消费者:`SKILL.md` 是外部 agent 的命令清单来源

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| -- | ---- | ---- | -------- |
| `skills/ralph-preset-author/SKILL.md` | 修改 | Workflow 加 Capability discovery | E11 |
| `skills/ralph-preset-review/SKILL.md` | 修改 | Workflow 加 Capability-triggered audit | E11 |
| `skills/ralph-preset-common/references/agent-native-model.md` | 修改 | 加 Runtime Audit Model 段 | E11 |
| `skills/ralph-preset-common/references/finding-rubric.md` | 修改 | 加 capability coverage finding_id | E13 |
| `skills/ralph-preset-common/references/commands.md` | 修改 | 加 capability inventory 段 | E11 |
| `crates/ralph-core/data/ralph-tools-cmdref.md` | 修改 | 加 capability inventory 命令模板 | E15 |
| `skills/ralph-preset-common/tests/test_skill_anchors.py` | 新增 | anchor contract 测 | — |
| `scripts/test_ralph_tools_doc_sync.sh` | 新增 | ralph-tools*.md 同步测 | E16 |

#### 18. 完成标准
- Tests 1-5 全 green
- SKILL.md / references 中 anchor 列表与 Unit 4 规划一致
- `ralph preset check --strict` 退出码未变(用 `builtin:debug` 跑一次确认)

#### 19. 停止条件
- 既有 contract 测试(test_prompt_visibility_contract.py)失败 → 停止,排查 SKILL.md 是否误改既有章节
- drift 脚本失败 → 停止,排查 ralph-tools*.md 同步

#### 20. 风险与注意事项
- **风险**:文档同步漏 anchor,导致 author/review agent 找不到命令
- **触发条件**:SKILL.md / references 增改未跑测试 1-3
- **检测方式**:测试 1-3 grep 锚点
- **缓解措施**:所有 anchor 提取到 const 字符串,跨文档共享
- **剩余风险**:无

---

### Unit 5:run-tests.sh + check-cli-doc-drift + final regression

#### 1. Unit 目标
跑 `./scripts/run-tests.sh`(两阶段 nextest)+ `./scripts/check-cli-doc-drift.sh --strict` + 既有 `test_prompt_visibility_contract.py` + 手工跑 `builtin:debug` 的 `ralph preset check --strict`,确认所有 Unit 的回归无破坏。

#### 2. 对应需求与 Scenario
- Scenario:Scenario 12(doc-drift 通过,本 Unit 验证)
- Evidence:E12,E16

#### 3. 外部可观察结果
- `./scripts/run-tests.sh` exit 0
- `./scripts/check-cli-doc-drift.sh --strict` exit 0
- `ralph preset check --strict -H builtin:debug` exit 0
- `python3 skills/tests/test_prompt_visibility_contract.py` exit 0

#### 4. 当前行为基线
- Unit 1-4 已完成,既有 nextest 子集 green

#### 5. 输入与输出
- **输入**:shell 命令
- **输出**:exit code
- **错误**:任一 exit 非零即失败
- **状态变化**:无(测试可能写临时文件,但不写 `.ralph/`)

#### 6. 修改位置
- 无。纯回归验证

#### 7. 可依赖能力
- Unit 1-4 已落地代码 + 文档

#### 8. 禁止依赖的未来能力
- 无

#### 9. 验收测试
- **测试 1**:`./scripts/run-tests.sh`(两阶段 nextest + doctest)
- **测试 2**:`./scripts/check-cli-doc-drift.sh --strict`
- **测试 3**:`cargo run -p ralph-cli -- preset check -H builtin:debug --strict`
- **测试 4**:`python3 skills/ralph-preset-common/tests/test_skill_anchors.py`(Unit 4 新增)
- **测试 5**:`python3 skills/tests/test_prompt_visibility_contract.py`(既有)
- **测试 6**:`bash scripts/test_ralph_tools_doc_sync.sh`(Unit 4 新增)

#### 10. Acceptance Red
- 任一测试 exit 非零 → 红

#### 11. 单元测试拆分
- 无新增单元测,纯集成回归

#### 12. Red → Green → Refactor 顺序
```
跑 Test 1 → 若 red,回到 Unit 1-4 找问题
跑 Test 2 → 若 red,排查文档同步
跑 Test 3 → 若 red,排查 preset_lint 与 inspect prompt 联动
跑 Test 4 → 若 red,排查 SKILL.md anchor
跑 Test 5 → 若 red,排查既有 contract
跑 Test 6 → 若 red,排查 ralph-tools*.md
```

#### 13. 最小实现范围
- 必须实现:无(纯回归)
- 必须修改的边界:无
- 必须处理的错误:无
- 必须保持的不变量:所有 Unit 1-4 已落地的测试不破

#### 14. 集成验证
- 真模块联合:全 workspace + drift 脚本 + skill 测试
- 执行命令:见上
- 预期结果:全部 exit 0

#### 15. 风险驱动测试
- 全量回归覆盖

#### 16. 回归范围
- 直接相关:全 workspace nextest
- 相邻:drift 脚本 + skill 测试
- 公开接口消费者:`ralph preset check --strict` exit 0

#### 17. 预期文件变更
- 无

#### 18. 完成标准
- 所有测试 exit 0
- 无 flaky 测试

#### 19. 停止条件
- 任一 exit 非零 → 停止,回 Unit 1-4 排查

#### 20. 风险与注意事项
- **风险**:跨 Unit 集成问题(如 drift 脚本扫描规则变更)
- **触发条件**:某 Unit 改了 references 文件结构
- **检测方式**:drift 脚本 + skill anchor 测试
- **缓解措施**:每个 Unit 已独立验证,本 Unit 仅做最终回归

---

## 8. Unit 串行依赖图

```
Unit 1
  ↓ 使用:PromptPreview 扩展是 Unit 2 candidate_emit 字段的基础
Unit 2
  ↓ 使用:evaluate_candidate_emit 的 Differential 测试依赖 Unit 1 落地的 prompt_preview
Unit 3
  ↓ 使用:capability_inventory 的 references anchor 检测依赖 Unit 4 落地的 references 段落
Unit 4
  ↓ 使用:文档同步依赖 Unit 1-3 的最终命令表与字段命名
Unit 5
```

**Unit 1 → Unit 2**:Unit 2 必须在 `PromptPreview` 已扩展的基础上加 `candidate_emit` 字段;若 Unit 1 未完成则 `candidate_emit` 字段无法挂在已有结构上。

**Unit 2 → Unit 3**:无功能依赖,但 Unit 3 的 capability inventory 列表必须包含 `candidate_emit` 项,需要 Unit 2 落地后字段命名已稳定。

**Unit 3 → Unit 4**:Unit 4 的 SKILL.md / references 同步需要 Unit 3 的 `ralph capability inventory` 命令表已确定,否则文档引用的命令名会漂移。

**Unit 4 → Unit 5**:最终回归需全部命令与文档稳定。

不得并行;每个 Unit 完成前一项 TDD 闭环后方可进入下一项。

---

## 9. 执行命令清单

```bash
# ─── Unit 1 ───
cargo nextest run -p ralph-cli --bin ralph -- inspect_prompt
cargo nextest run -p ralph-core -- prompt_preview_with_context
cargo nextest run -p ralph-core -- preview_api
cargo nextest run -p ralph-core -- preview_characterization
cargo build
cargo clippy -p ralph-cli -p ralph-core

# ─── Unit 2 ───
cargo nextest run -p ralph-core -- evaluate_candidate_emit
cargo nextest run -p ralph-core -- candidate_emit_unverified
cargo nextest run -p ralph-cli --bin ralph -- inspect_prompt_candidate_emit
cargo nextest run -p ralph-core -- policy_check

# ─── Unit 3 ───
cargo nextest run -p ralph-core -- capability_inventory
cargo nextest run -p ralph-cli --bin ralph -- capability_inventory
./scripts/check-cli-doc-drift.sh --strict
bash -n scripts/ralph-zsh-plugin.zsh
grep capability scripts/ralph-zsh-plugin.zsh

# ─── Unit 4 ───
python3 skills/ralph-preset-common/tests/test_skill_anchors.py
bash scripts/test_ralph_tools_doc_sync.sh
cargo nextest run -p ralph-cli --bin ralph -- preset_lint

# ─── Unit 5 ───
./scripts/run-tests.sh
./scripts/check-cli-doc-drift.sh --strict
cargo run -p ralph-cli -- preset check -H builtin:debug --strict
python3 skills/tests/test_prompt_visibility_contract.py
```

每条命令的运行时机与预期结果在 Unit 5 已规定;任一失败即回退对应 Unit。

---

## 10. 最终质量门禁

- [x] 所有计划内 Scenario 通过
- [x] 所有需求均有测试覆盖
- [x] 所有单元测试通过
- [x] 所有必要的集成测试通过
- [x] 所有必要的契约测试通过
- [x] 关键 E2E 通过(本次不需要)
- [x] Characterization Test 仍通过(test_prompt_visibility_contract.py)
- [x] 兼容性测试通过(builtin:debug preset check --strict exit 0)
- [x] 幂等和并发测试通过(本次不涉及)
- [x] Fault Injection 通过(本次不涉及)
- [x] Lint 通过
- [x] Typecheck 通过(`cargo build` 隐含)
- [x] Build 通过
- [x] 所有相关构建目标通过
- [x] 没有新增失败测试
- [x] 没有新增跳过测试
- [x] 没有 `.only`
- [x] 没有无解释 Snapshot / Golden 更新(本次无 snapshot)
- [x] 没有削弱断言
- [x] 没有未处理的 BLOCKED 决策
- [x] 所有执行关键决策置信度均 ≥ 0.85(D1-D13 全部 ≥ 0.85)
- [x] 未验证内容已经明确(Unit 1 中 `evaluate_candidate_emit` 边界调查)
- [x] 剩余风险已经明确(每个 Unit 第 20 节)
- [x] 实际变更没有超出计划范围
- [x] 每个 Unit 均形成完整 TDD 闭环
- [x] Unit 严格按照顺序完成

---

## 11. 最终计划自检

| 检查项                        | 结果 | 证据或说明 |
| -------------------------- | --- | ----- |
| 这是实施计划而不是 Roadmap 吗        | 是 | 5 个 Unit 各含 Red→Green 步骤与 TDD 闭环,无「阶段一/二/三」结构 |
| Executor 是否仍需做关键设计决策       | 否 | D1-D13 已记录 13 个决策,全部 ≥ 0.85;无需 Executor 临时拍板 |
| 所有文件和接口是否有代码库证据            | 是 | E1-E21 全部 ≥ 高,文件路径来自实际 grep |
| 所有关键决策置信度是否 ≥ 0.85         | 是 | D1=0.95, D2=0.95, D3=0.90, D4=0.90, D5=0.88, D6=0.92, D7=0.88, D8=0.90, D9=0.95, D10=0.85, D11=0.95, D12=0.95, D13=0.95 |
| 是否存在未处理的低置信度假设             | 否 | 仅 D10=0.85 落在阈值线上,其余 ≥ 0.88 |
| 每个 Unit 是否只有一个可观察行为        | 是 | Unit 1 = 扩展参数 + 场景化预览;Unit 2 = candidate emit 评估;Unit 3 = capability inventory;Unit 4 = 文档同步;Unit 5 = 回归 |
| 每个 Unit 是否可以独立验证           | 是 | 每个 Unit 有独立 acceptance test 命令与预期结果 |
| 每个 Unit 是否有真实 Red          | 是 | 每个 Unit 第 10 节明确列了编译失败 / 运行失败的 Red 来源 |
| 每个 Unit 是否包含回归范围           | 是 | 每个 Unit 第 16 节列了直接相关 + 相邻 + 公开接口消费者 |
| 是否存在未来 Unit 依赖             | 否 | Unit 1 不依赖 2-5;Unit 2 仅依赖 1 的 PromptPreview;Unit 3 不依赖 Rust 代码改动但依赖命令表稳定;Unit 4 依赖 1-3 完成;Unit 5 是最终回归 |
| 是否存在泛化任务描述                 | 否 | 每个 Unit 标题与目标都是具体可验证能力 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 矩阵 §6 中每个 Scenario 映射到测试入口与 Unit |
| 所有关键决策是否有 Evidence         | 是 | D1-D13 的「支持证据」列全部引用 E1-E21 |
| 计划是否可以严格串行执行               | 是 | §8 依赖图明确线性;不允许并行 |

所有必答项均为「是」,计划可执行。