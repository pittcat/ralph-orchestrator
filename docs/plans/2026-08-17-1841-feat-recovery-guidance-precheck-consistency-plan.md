---
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
depth: deep
research_complete: 2026-08-17
---

# Precheck gate 与 payload consistency 的可配置恢复指导

## Goal Capsule

本计划把现有语义拒收的“失败原因 + 通用修复指导 + 失败检查单元专用指导”传递给真正负责重试的 agent。目标是让 event_loop.precheck 与 event_policy.payload_consistency 使用同一套 correction/prompt 数据通道；失败后仍由现有 bounded retry 路由回原 target hat，agent 必须基于新证据修复并重新执行 ralph emit --policy-check，不能通过改写 payload 绕过门禁。

本计划同时修正 red-team-attack 中高风险只读 hat 的 workspace 纪律：由 hat 自己在关键入口/出口执行 workspace precheck 并把证据保存到 .ralph/red-team/**；发现 agent 在本 activation 产生了生产代码或其它越界变更时，现有 precheck gate 拒绝并 retry，下一轮 target hat 必须先处理可证明属于本 activation 的变更，然后重新核验。runtime 不新增通用 worktree 状态采集、自动 reset、stash、clean 或回滚。

执行顺序固定为 U1 → U2 → U3 → U4 → U5 → U6 → U7。每个 Unit 必须在当前验收、单元测试、集成测试、回归、Build/Lint 检查完成后才能进入下一 Unit。

## Product Contract

### 0. 计划状态

- 状态：READY。所有实施关键决策均有直接源码、测试、配置或历史方案证据，置信度不低于 0.85；没有 BLOCKED 决策。
- 代码库基线：分支 pittcat-dev，HEAD 328173ac；调查时工作树干净。
- 调查范围：ralph-core config、precheck gate runner、event-policy evaluator、correction renderer、scenario 测试；red-team-attack preset 及 schema/author notes；ralph-preset-author、ralph-preset-review；注入给 agent 的 data skill；相关历史计划和提交。
- 已执行只读验证：git status --short --branch、git rev-parse HEAD、rg/sed 检查入口/测试/文档、相关 git log。调查开始时工作树干净；写计划期间工作树出现了并行用户修改（见当前 git status），本计划只新增本文件，不覆盖、不整理这些修改。按 ce-plan 约束，本阶段没有运行 build、lint 或测试，也没有修改生产代码。
- 尚未执行验证：所有 Red/Green、BDD、preset lint、skill anchor、CLI doc drift、Build、Clippy、全量 ./scripts/run-tests.sh；这些由 Coding Agent 严格按 Unit 顺序执行。
- 本文件是实现计划，不是本轮实现；计划之外不得扩大到 runtime workspace monitor 或自动恢复系统。

### 1. 功能目标

#### 业务目标、调用方与行为差异

- 调用方：使用 event_loop.precheck 的 producer hat、使用 event_policy.payload_consistency 的业务事件 producer、合成的 precheck-X gate hat，以及 preset 作者和 reviewer。
- 当前行为：precheck rejected payload 含 failed_checks/reason，runtime 已建立 semantic CorrectionContext/evidence；consistency 命中已建立 observed/invariant/proof evidence。两者都有通用 anti-cheat recovery prose，但 preset YAML 不能声明通用修复步骤，也不能声明某个 check unit 的专用修复步骤。
- 目标行为：失败的下一轮 target hat 在 ORCHESTRATOR CORRECTION 中同时看到原有结构化失败原因/evidence、preset 配置的 recovery_guidance.common、与实际失败 check 对应的 recovery_guidance.by_check。
- 关键差异：只在命中 gate 且配置了 guidance 时增加内容；缺少新字段时输出与现状一致。semantic rejection 仍不显示 expected_payload_template、required_fields、suggested_payload_shape 或 suggested_command 作为业务成功模板。

#### 输入、输出、状态与不变量

- 输入：
  - PrecheckRule.prompt 的 1-based checklist index、rejection.failed_checks、rejection.synthetic；
  - PayloadConsistencyRule.id、message、when 和当前 payload；
  - 两类 rule 下可选的 recovery_guidance.common: string[] 与 recovery_guidance.by_check: map<string,string[]>。
- 输出：
  - correction prompt 中有界、经安全展示处理的通用恢复指导和检查单元专用指导；
  - CLI --policy-check 的结构化 evidence 与 guidance 同源；
  - 原有 task.resume 路由字段、retry count、target hat、exhaustion topic 不变。
- 状态变化：只增加 correction/evidence 内容；不增加事件 topic，不改变 retry counter key，不改变 accepted/rejected/blocked 状态机。
- 副作用：继续使用现有 ledger/recovery 记录；guidance 本身不写 workspace。red-team 的 workspace precheck 证据由 hat 写入 .ralph/red-team/**，不由 runtime 自动写入。
- 不变量：guidance 不改变 gate 判定；不能把 synthetic 的“未观察”伪装成真实失败 check；不能改变 semantic/mechanical 分流；不能被无关 hat 看见或消费。
- consistency 的下一轮仍沿现有 CorrectionContext/PromptContext re-entry 工作，不额外发布第二个 task.resume；真实 evidence-bound correction scenario 已用同一 producer 的后续 mock activation 证明该路径。若 U5 发现真实 runner 在某个 execution mode 下不能重新激活原 producer，必须停止 U4/U5、补充调用链证据并重新决策，不能静默新增重复 resume。

#### 错误、兼容、性能与安全

- 无效 guidance key、空 key、超长/控制字符 guidance 在 preset lint 中报告；strict lint 阻断启动，默认 lint 按现有规则给 Warn。运行时仍用 safe_display 做最后边界保护。
- precheck by_check key 必须是 1..=prompt.len() 的十进制字符串；普通 rejection 只取 failed_checks 中明确命中的 key；synthetic 只显示 common，不显示具体 check guidance。
- consistency by_check key 必须等于当前 rule 的稳定 id；命中 rule 时选择该 id 的专用指导。未声明该 key 时只显示 common。
- 新字段全为 serde(default)，旧 preset 不需要迁移；省略 guidance 等价于当前输出。
- 不新增依赖、不访问网络、不读取 runtime ledger 生成 guidance；prompt 追加内容使用现有 correction 预算和 safe-display 限制。
- guidance 是 preset 作者提供的提示，不是 runtime 事实；prompt 中必须把它和 Observed/Invariant/Must re-prove 分段。
- 本功能不提供 semantic suggested_command 配置字段。验证命令只能作为作者写入 common/by_check prose 的动作，仍必须先通过 ralph emit <topic> --policy-check。

#### 范围与非目标

本次范围：

- 新增可解析、可 lint、可渲染的 guidance 配置契约；
- precheck 与 payload consistency 共享 correction/evidence/prompt 传递；
- 保留失败原因、通用 runtime recovery prose、目标 hat、retry/exhaustion 语义；
- 为 red-team-attack 的高风险 hat 增加 workspace precheck/证据/失败 retry 指令和 guidance 示例；
- 同步 ralph-preset-author、ralph-preset-review、agent-facing data skill、human guide、fixtures/anchor 测试。

非目标：

- 不在 runtime 新增通用 worktree snapshot、dirty-state watcher、自动 revert/reset/stash/clean；
- 不改变 precheck gate 的 official 名称；文档 prose 使用“precheck gate”，现有 author notes 字段 precheck_guard 继续作为选择字段并明确其含义；
- 不增加新的 task.resume topic 或另一套 retry 通道；
- 不把 payload consistency 改成跨事件历史检查；
- 不把 semantic rejection 变成 schema replacement；
- 不把 red-team-attack 改成允许生产代码修改的 preset；
- 不新增 builtin preset，不改 preset 名称，因此不改 manifest、index 或 zsh builtin 名称补全。

已确认假设：

- 当前 correction prompt 是统一的目标注入点；precheck 和 policy rejection 都已经通过 CorrectionContext 进入 prompt。
- EvidenceDetail 已是 precheck/consistency 的共同结构化载体；扩展它比新建 parallel resume payload 更小。
- failed_checks 可保留现有数字/string identity 兼容处理，只把最终匹配 key 规范化为字符串。

待验证但不阻塞的实现细节：

- PolicyFinding.evidence 到 CLI JSON 的具体序列化投影是否由同一 helper 完成；U4 先沿现有调用链定位，若不同，必须让两者共享 normalized guidance builder。
- red-team 各高风险 hat 当前保存证据的具体 .ralph/red-team 子路径；U6 先读取完整 instructions，按现有 artifact convention 选择，不让 Executor 猜路径。

### 2. 代码库现状与证据

#### 2.1 当前实现入口与调用链

1. YAML 反序列化为 RalphConfig；event_loop.precheck 类型在 crates/ralph-core/src/config/precheck.rs，event_policy.payload_consistency 类型在 crates/ralph-core/src/config/event_policy.rs。
2. RalphConfig::apply_precheck_desugar 在 crates/ralph-core/src/config/ralph_config.rs 把 X producer 改成 X.proposed 并合成 precheck-X；build_gate_instructions 把 prompt 渲染到 gate hat。
3. EventLoop::handle_precheck_rejection 在 crates/ralph-core/src/event_loop/parse_and_emit/step_dispatch.rs 调用 precheck_gate_runner::build_precheck_evidence，再把 semantic/evidence 写入 correction block。
4. validate_event_with_options 在 crates/ralph-core/src/event_policy/validation.rs 命中 consistency rule 时从当前 payload 采集 referenced fields，构造 PolicyFinding.evidence。
5. event_loop/policy.rs 对 PolicyFinding.evidence 升级刚创建的 CorrectionContext 为 semantic，并替换 prompt 队列中的条目。
6. PromptContext 负责 target-aware consume；target_hat 让 correction 只给目标 hat，且无关 hat 构建 prompt 不会吞掉目标条目。

#### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | config/precheck.rs::PrecheckRule | 当前只有 prompt 与 on_fail；on_fail.reason 只是短原因。 | guidance 必须加在 rule 配置上，不能把长指导塞进 reason。 | 高 |
| E2 | config/event_policy.rs::PayloadConsistencyRule | 当前只有 id/topic/when/message。 | guidance 必须成为 rule 的可选配置，不改变 predicate AST。 | 高 |
| E3 | config/ralph_config.rs::apply_precheck_desugar/build_gate_instructions | gate 是 runtime 合成的；check identity 目前是 1-based checklist index。 | by_check 使用字符串化 1-based index，并由 lint 校验范围。 | 高 |
| E4 | event_loop/precheck_gate_enforcement.rs | rejected.failed_checks 是 gate 反馈来源；synthetic 标记 silent/ambiguous。 | 普通 reject 选择报告 unit；synthetic 禁止选择 specific。 | 高 |
| E5 | step_dispatch.rs::handle_precheck_rejection | precheck rejection 已进入 emit_correction_context，再用 evidence 升级 semantic。 | 不新增 resume 通道，只补 normalized guidance。 | 高 |
| E6 | event_loop/policy.rs | policy finding evidence 已进入 CorrectionContext，按 target hat 替换 block。 | consistency 复用同一渲染/路由/消费路径。 | 高 |
| E7 | correction/mod.rs::EvidenceDetail | 统一保存 observed/invariant/proof/synthetic，semantic renderer 已隐藏 replacement fields。 | guidance 放在 evidence-bound correction 旁，保持 semantic 不变量。 | 高 |
| E8 | event_policy/validation.rs 1823 附近 | consistency 只评估当前 payload，按 declaration order 命中首条。 | guidance 选择绑定当前 rule id，不能读取历史。 | 高 |
| E9 | precheck_gate_runner.rs tests | 已覆盖 malformed、synthetic、numeric/string check identity 与 retry payload。 | 新测试守住旧 failed_checks/reason 语义，只新增 guidance 选择。 | 高 |
| E10 | tests/scenarios/payload_consistency/evidence_bound_precheck_routing.yml | 真 EventLoop 已覆盖 precheck target routing 和 synthetic evidence。 | 在现有场景上增加 guidance 断言，不另造 stub。 | 高 |
| E11 | tests/scenarios/payload_consistency/evidence_bound_correction.yml | 真 EventLoop 已覆盖 consistency reject → correction → corrected payload accepted。 | 在同一场景加入 common/specific guidance 断言。 | 高 |
| E12 | docs/guide/precheck-gates.md、payload-consistency.md | 只描述现有字段和通用 recovery，没有 custom guidance。 | 文档增加 YAML 形状、选择规则、synthetic 和停止条件。 | 高 |
| E13 | crates/ralph-core/data/ralph-tools-{precheck,recovery-directives,emit}.md | 已说明 semantic evidence、suggested_command 禁用和 bounded retry，但没有 custom common/specific guidance。 | 必须描述 trigger、action、source、stop，不泄漏 runtime internals。 | 高 |
| E14 | skills/ralph-preset-author/references/*、review/references/* | 已有 key-stage guard、retry budget、payload lint、evidence-bound rubric。 | 两套 skill 同步新字段，并保持 guard 与 official gate 边界。 | 高 |
| E15 | skills/ralph-preset-review/tests/test_skill_anchors.py 与 fixtures | 已有 finding、key-stage、evidence-bound fixture/anchor 检查。 | 新 contract 增加正/负 fixture 或等价 anchor 断言。 | 高 |
| E16 | presets/en/red-team-attack.yml 与 author notes | 已声明只读生产树但历史路径产生过未跟踪源码文件。 | 高风险 hat 增加 workspace precheck 和 guidance；仍由 gate retry。 | 高 |
| E17 | preset_lint/mod.rs、payload_consistency.rs、finding_id.rs | lint 集中注册 finding ID、strictness 和 ALL_FINDING_IDS。 | guidance 合法性加入同一 preset_lint 家族。 | 高 |
| E18 | achieved evidence-bound plan 与提交 5371bab5/6b49ce04/f383d115 | 已确立 evidence-bound semantic/mechanical split、target correction、retry/exhaustion。 | 只扩展 guidance，不复制或回退 correction 架构。 | 中-高 |
| E19 | tests/scenarios/payload_consistency/evidence_bound_correction.yml 的 mock sequence | 当前 consistency rejection 后同一 fixer 在后续 activation 再次收到 prompt，修复后的 payload 可被接受；该场景没有额外 task.resume topic。 | consistency guidance 复用现有 correction-only re-entry；U5 必须继续验证 target re-entry，不新增第二套 resume。 | 高 |

#### 2.3 受影响范围

- 生产配置：config/precheck.rs、config/event_policy.rs、config/mod.rs。
- 生产运行时：event_loop/precheck_gate_runner.rs、parse_and_emit/step_dispatch.rs、event_loop/policy.rs、event_policy/validation.rs、correction/mod.rs。
- 静态检查：preset_lint/mod.rs、payload_consistency.rs、finding_id.rs，以及必要时新增同目录 guidance lint module。
- 测试：config 单测、precheck runner、correction、event policy、payload consistency/precheck BDD、preset lint、author/review skill tests。
- preset：presets/en/red-team-attack.yml、presets/schemas/red-team-attack.yml、red-team-attack-preset-author-notes.md。
- agent-facing：ralph-tools-precheck.md、ralph-tools-recovery-directives.md、ralph-tools-emit.md。
- human docs：docs/guide/precheck-gates.md、payload-consistency.md、preset-authoring.md。
- operator skills：skills/ralph-preset-author/**、skills/ralph-preset-review/**，仅使用 AGENTS 允许范围。
- 未确认且不应擅自加入：runtime workspace snapshot、数据库、外部 API、CLI 新子命令、前端 UI。

### 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | guidance 走哪条链？ | 新 task.resume 字段/新 topic；扩展已有 evidence/correction | 扩展 EvidenceDetail，由 PolicyFinding/precheck evidence 进入 CorrectionContext 和 prompt；CLI JSON复用同一 normalized builder | E5-E8/E18 | 新 topic 会复制 retry/target/consume；task.resume 不是 prompt 权威恢复面 | 0.96 |
| D2 | 配置形状？ | common+single specific；common+by_check；两类各自设计 | 统一 recovery_guidance: common string[] + by_check map<string,string[]>；precheck key 为 1-based index，consistency key 为 rule id | E1-E4/E8 | single specific 无法表达多 checklist；两套字段会制造双合同 | 0.91 |
| D3 | synthetic 如何选择 specific？ | all check；把 failed_checks 当事实；只 common | 只给 common，不注入 by_check；保留 synthetic/unchecked evidence | E4/E9/E10 | synthetic 没有 fact-checked result，specific 会制造证据 | 0.98 |
| D4 | semantic 是否新增 suggested_command？ | 任意 command；verification-only command；不新增 | 不新增字段；验证动作只能以 prose guidance 出现，最终仍须 policy-check | E7/E13/E18 | semantic contract 禁止 replacement command；新字段易变成功模板 | 0.93 |
| D5 | 安全边界？ | 只 safe_display；只 lint；lint+render | preset lint 检查 key/空值/控制字符/长度，renderer 仍 safe_display，输出有界 | E7/E12/E13/E17 | 一层保护无法覆盖动态/绕过 lint config | 0.94 |
| D6 | runtime 是否采集 workspace？ | 通用 snapshot；preset 自己 precheck；不保护 | 不加 runtime collection；red-team hat 自行 entry/exit precheck，gate rejection 回 target | E16、用户非目标 | 通用 runtime 改变状态边界并引入回归，用户明确不希望 | 0.97 |
| D7 | guidance lint 放哪？ | 不 lint；只 skill review；加入 preset_lint | 加入 preset_lint，复用 strictness/finding ID/ALL_FINDING_IDS 和两套 skill | E14/E15/E17 | skill 不能替代启动前结构化 lint | 0.90 |
| D8 | 哪些 red-team hat？ | 所有；单一事故 hat；所有明确只读但执行 helper/实验的 producer edge | target-locker、plan-resolver、attack-surface-mapper、experiment-plan-validator、experiment-runner | E16 与 preset instructions | 覆盖事故路径及相邻 mutation surface，不误标纯 consumer | 0.88 |
| D9 | consistency rejection 是否再发布一个 task.resume？ | 保持现有 correction-only re-entry；新增 task.resume；由 preset 自己重发 | 保持现有 correction-only re-entry；guidance 随 EvidenceDetail 进入下一次 target prompt，retry/escalation 继续使用现有 correction counter | E6、E11、E19 | 现有真实场景已经证明无额外 task.resume 也能重新激活 fixer；新增 resume 会产生重复恢复、双 budget 或双 correction 风险 | 0.90 |

低于 0.90 的 D8/D9 仍达到执行阈值，但 U5 必须用真实 runner 验证 consistency correction 能重新激活原 producer；U6 必须先读取五个 hat 的完整 instructions、publishes/triggers、现有 artifact path。若发现 execution mode 下没有 re-entry、合法生产写入或不同责任边界，停止并更新 D8/D9，不得盲改。

## Planning Contract

### 4. BDD 行为规格

Feature: Gate rejection recovery guidance

  Background:
    Given a preset uses the existing semantic correction channel
    And the target hat is the existing retry target

  Scenario: precheck rejection injects common and failed-check guidance
    Given a precheck rule has common guidance and by_check guidance for check "1"
    And the gate emits a non-synthetic rejection with failed_checks containing "1"
    When runtime routes the rejection to on_fail.target
    Then target prompt contains failure reason, common guidance, and check "1" guidance
    And the prompt still contains Observed/Invariant/Must re-prove evidence
    And an unrelated hat does not receive or consume the correction

  Scenario: synthetic precheck rejection does not fabricate a failed check
    Given a precheck gate is silent or ambiguous
    When runtime creates the existing synthetic rejection
    Then target prompt marks gate_silent_or_ambiguous/unchecked
    And prompt contains common guidance but no by_check guidance
    And retry routing/budget are unchanged

  Scenario: consistency rejection selects matching rule guidance
    Given a consistency rule has id rule-a and by_check key rule-a
    When current payload hits rule-a
    Then correction contains rule-a message/invariant, common guidance and rule-a guidance
    And guidance from another rule is absent
    And replacement payload and suggested command are absent

  Scenario: guidance is absent without legacy behavior change
    Given a rule has no recovery_guidance block
    When gate rejects an event
    Then reason, evidence, target, retry count and recovery prose remain
    And no empty guidance headings render

  Scenario: malformed guidance is rejected before runtime
    Given a preset has an empty key, bad key, unsafe text or oversized text
    When strict preset lint runs
    Then a stable recovery-guidance finding is returned and startup is rejected
    And no event is emitted

  Scenario: red-team read-only hat changes production workspace
    Given a high-risk hat records entry workspace evidence
    And exit evidence finds a production or non-.ralph untracked mutation
    When it attempts its guarded terminal event
    Then the precheck rejects with the changed workspace check identified
    And existing retry routes to the responsible target
    And runtime does not reset, stash, clean or delete files

  Scenario: retry fixes only a provable activation-owned mutation
    Given target hat receives the workspace mutation rejection
    When it can prove which file it created in the current activation
    Then it removes/reverts only that mutation, reruns checks, policy-checks, and re-emits
    And when ownership is uncertain it stops instead of broad cleanup

### 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 测试层级 | 风险补充 | E2E |
|---|---|---|---|---|---|
| S1 precheck common/specific | reason、common、failed-check guidance、evidence、target isolation 全在 | precheck_gate_runner + evidence_bound_precheck_routing.yml | 单元+真 EventLoop | old failed_checks/reason characterization | 否 |
| S2 synthetic | common 可见、specific 不可见、unchecked 保持 | runner + existing scenario | 单元+BDD | synthetic negative | 否 |
| S3 consistency | rule id 精确选择，其他 rule 不泄漏 | event policy tests + evidence_bound_correction.yml | 单元+BDD | first-hit order characterization | 否 |
| S4 legacy/off | 无 guidance 或 disabled 与基线一致 | config + preset parity | 单元+集成 | default-off no-op | 否 |
| S5 invalid config | strict lint 拒绝错误 key/unsafe/oversize | guidance lint tests | 单元+lint | boundary matrix | 否 |
| S6 semantic safety | guidance 不打开 replacement fields/commands | correction + policy-check tests | 单元+CLI集成 | malicious text | 否 |
| S7 red-team mutation | 越界变更 rejection/retry；无 runtime cleanup | real preset verify/scenario | 真 workflow | tracked/untracked/allowed path | 否 |
| S8 skills | author/review 审 guidance、workspace、retry、术语 | test_skill_anchors.py + fixtures | Python contract | wrong-key/replacement negative | 否 |
| S9 docs | agent guide 有 trigger/action/source/stop，human docs与代码一致 | doc drift + skill tests | static+tests | forbidden internal terms | 否 |

每个验收测试必须先形成真实 Red；编译损坏、命令错误、fixture 缺失或测试未执行目标逻辑都不算 Red。scenario 必须使用真实 EventLoop runner，不用只检查 iteration 数的 stub。

### 6. 需求—测试追踪矩阵

| Requirement | 需求 | Scenario | 验收测试 | 单元 | 集成/契约 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | 两类 rule 支持 common/by_check | S1/S3/S4 | parse/round-trip | config tests | preset lint | 否 | E1/E2 |
| R2 | precheck reason/common/specific 到 target | S1 | routing BDD | runner/renderer | real EventLoop | 否 | E3-E6 |
| R3 | synthetic 只 common | S2 | synthetic BDD | runner | real EventLoop | 否 | E4/E9/E10 |
| R4 | consistency 按 rule id | S3 | consistency BDD | validation/renderer | real EventLoop | 否 | E6/E8/E11 |
| R5 | semantic no replacement command/payload | S6 | prompt/JSON assertions | correction | CLI integration | 否 | E7/E13 |
| R6 | old/disabled no-op | S4 | characterization | config/normalize | preset parity | 否 | E1/E17 |
| R7 | guidance lint fail-close | S5 | lint negatives | lint | preset check | 否 | E17 |
| R8 | red-team self workspace precheck/retry | S7 | preset verify | 不新增collector | real preset | 否 | E16/D8 |
| R9 | agent guide executable | S2/S3/S9 | guide contract | anchor tests | doc drift | 否 | E13 |
| R10 | author/review sync | S8/S9 | fixtures/anchors | Python | review workflow | 否 | E14/E15 |
| R11 | no runtime collector/auto repair | S7 | absence review | no collector | preset verify | 否 | D6/E16 |

## Implementation Units

### Unit 1：建立 recovery guidance 配置契约与 preset lint

#### 1. Unit 目标

让旧 YAML 继续解析，同时让两类 rule 接受可选 recovery_guidance.common/by_check，并在 strict preset lint 中拒绝非法 key 和不安全文本。

#### 2. 对应需求与 Scenario

R1、R6、R7；S4、S5；D2、D5、D7；E1、E2、E17。

#### 3. 外部可观察结果

ralph preset check --strict 能识别 guidance 合同错误；未声明 guidance 的 preset 的 parse/normalize/lint 结果不变；本 Unit 尚不改变 prompt。

#### 4. 当前行为基线

PrecheckRule 和 PayloadConsistencyRule 没有 guidance 字段（E1/E2），现有 payload consistency lint 在 preset_lint/mod.rs 注册（E17）。先增加旧最小 YAML round-trip/no-op characterization。

#### 5. 输入与输出

输入为两类 rule 的 common 和字符串 map。输出为 typed config 和稳定 finding。错误包括空 key/空 item、precheck 非正数或越界 key、consistency key 非当前 id、控制字符/ANSI/零宽/超过 MAX_RULE_MESSAGE_BYTES。无配置不产生 finding。

#### 6. 修改位置

- 修改 crates/ralph-core/src/config/precheck.rs：增加 guidance field 和 parse tests；不改 desugar/retry/gate decision。
- 修改 crates/ralph-core/src/config/event_policy.rs：增加 guidance field 和 parse tests；不改 predicate evaluator。
- 修改 crates/ralph-core/src/config/mod.rs：导出 shared type。
- 新增 crates/ralph-core/src/preset_lint/recovery_guidance.rs（进入 Unit 前确认命名）：纯 config lint，复用现有 severity。
- 修改 preset_lint/mod.rs、finding_id.rs：注册 family、ALL_FINDING_IDS、排序。
- 扩展现有 config/lint tests；不新增只匹配完整 preset 文案的测试。

#### 7. 可依赖能力

serde defaults、safe_display::MAX_RULE_MESSAGE_BYTES、既有 unsafe message lint、LintStrictness。

#### 8. 禁止依赖的未来能力

不得修改 CorrectionContext、prompt、precheck runner、consistency evaluator、red-team preset 或文档。

#### 9. 验收测试

- precheck common/by_check parse + round-trip；
- consistency common/by_check parse + rule id；
- omitted block default/no-op；
- invalid key/unsafe/oversized text finding；
- default vs strict severity。

运行：cargo nextest run -p ralph-core -- recovery_guidance precheck payload_consistency；cargo nextest run -p ralph-core -- preset_lint。

#### 10. Acceptance Red

先运行新增 parse/lint tests。当前 struct 没有 guidance，预期出现字段/断言缺失和非法 guidance 未产 finding。若是 fixture/语法/未执行失败，停止，不算 Red。

#### 11. 单元测试拆分

1. 两类 rule parse/round-trip。
2. legacy omitted default。
3. empty/unsafe/oversized text。
4. precheck non-positive/out-of-range key。
5. consistency unknown key。
6. default/strict severity。

#### 12. Red → Green → Refactor 顺序

parse Red → shared config type/fields → Green；legacy no-op Red → serde defaults → Green；invalid-key Red → lint helper/IDs/register → Green；安全文本 Red → 复用既有规则 → Green；稳定排序和 comments Refactor；相关回归。

#### 13. 最小实现范围

只完成配置承载和合法性门禁；不读取 guidance、不选 failed check、不改变 prompt。

#### 14. 集成验证

使用 RalphConfig::parse_yaml 和现有 preset lint 真实入口；验证 builtin 无 guidance 不产生非预期 finding。

#### 15. 风险驱动测试

Characterization + 空/超长/控制字符/非正数/未知 key 边界矩阵。

#### 16. 回归范围

config/precheck、config/event_policy、config/ralph_config parse/normalize；core preset_lint；builtin strict lint。原因是 serde/lint 变更可能污染 parity。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| config/precheck.rs | 修改生产/测试 | precheck guidance | E1 |
| config/event_policy.rs | 修改生产/测试 | consistency guidance | E2 |
| config/mod.rs | 修改导出 | shared type | E1/E2 |
| preset_lint/recovery_guidance.rs | 新增生产/测试 | legal config | E17 |
| preset_lint/mod.rs、finding_id.rs | 修改 lint | stable IDs/register | E17 |

#### 18. 完成标准

parse/lint scenarios、legacy characterization、相关 nextest、Build、Clippy/format 全通过；无 prompt 行为变化；Unit 可独立提交。

#### 19. 停止条件

YAML key 不能稳定表达、safe-display 与 lint 冲突、或无配置 lint 结果改变时，停止并更新 D2/D5/D7。

#### 20. 风险与注意事项

重点防 YAML key 类型、只 lint 一类 rule、default 噪声。用 round-trip、双 rule 正负 fixture、全量 preset lint 检测。

### Unit 2：把 guidance 接入 evidence-bound correction 与安全渲染

#### 1. Unit 目标

把已解析 guidance 绑定到 EvidenceDetail/correction block，显示 common 和已选 specific，同时保持 semantic rejection 不显示替代 payload/command。

#### 2. 对应需求与 Scenario

R2、R3、R5、R6；S1、S2、S4、S6；D1、D3、D4、D5；E5-E9/E18。

#### 3. 外部可观察结果

CorrectionContext::render_block 在 evidence 后出现非空 guidance 段；target hat 可见、其它 hat 不可见；无 guidance 的旧 block 不增加空 heading。

#### 4. 当前行为基线

EvidenceDetail 只有 observed/invariant/proof/synthetic；semantic renderer 有固定 recovery prose；PolicyFinding.evidence 已进入 correction。现有 correction tests 作为 characterization。

#### 5. 输入与输出

输入是 normalized RecoveryGuidance 和 evidence。输出是 structured evidence + prompt 的 Common recovery guidance / Check-specific recovery guidance。缺 key 不显示 specific；synthetic 只 common；所有文本 safe_display。

#### 6. 修改位置

- correction/mod.rs：扩展 EvidenceDetail/renderer/serde；不改 gate selection/retry。
- correction/mod.rs tests：common/specific/synthetic/no-replacement/unsafe text。
- precheck_gate_runner.rs：提供/测试 normalized selection，不改变 payload schema。
- event_loop/policy.rs：传递 evidence guidance，不复制 renderer。

#### 7. 可依赖能力

U1 typed guidance、EvidenceDetail、safe_display、FeedbackKind::Semantic、target-aware PromptContext。

#### 8. 禁止依赖的未来能力

不得改 precheck YAML selection、consistency evaluator、red-team、skills、docs、CLI syntax。

#### 9. 验收测试

- semantic correction renders common/specific/evidence；
- synthetic common-only；
- semantic guidance 不渲染 replacement；
- malicious/oversize text 不破坏 block；
- correction serde round-trip；
- 运行 cargo nextest run -p ralph-core -- correction precheck。

#### 10. Acceptance Red

先运行 common/specific renderer tests；当前 EvidenceDetail 无 guidance，预期缺失。synthetic negative 若把所有 checklist 当 specific 也必须失败；struct literal 编译失败不算 Red。

#### 11. 单元测试拆分

common only、one/multiple specific ordering、missing key、synthetic suppression、semantic suppression、legacy/mechanical、target queue、safe-display。

#### 12. Red → Green → Refactor 顺序

model/render Red → optional evidence guidance + pure render helper → Green；synthetic Red → suppress branch → Green；no-replacement Red → preserve semantic gate → Green；serialization/order Red → deterministic defaults → Green；Refactor comments/labels；regression。

#### 13. 最小实现范围

只实现 shared normalized guidance 的存储/渲染/序列化；不从 raw YAML、last_message 或自然语言猜 check，不执行 command。

#### 14. 集成验证

用真实 PromptContext 构造两个 hat，验证 target partition 和 ledger/recovery append 未变。

#### 15. 风险驱动测试

安全边界和 target queue characterization；不做无依据的网络/concurrency 测试。

#### 16. 回归范围

core correction、event_loop rejection/correction、precheck runner、existing evidence-bound scenarios。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| correction/mod.rs | 修改生产/测试 | shared guidance render | E7/E18 |
| precheck_gate_runner.rs | 修改测试/helper | synthetic/selection contract | E4/E9 |
| event_loop/policy.rs | 修改生产/测试 | evidence propagation | E6 |

#### 18. 完成标准

renderer/serialization/target tests 与旧 semantic/mechanical tests 全绿；无 topic/retry 改动；Build/Lint/format 通过。

#### 19. 停止条件

EvidenceDetail 无法同时满足 correction prompt 和 CLI JSON、或需改 prompt builder 外部 API 时，停止更新 D1，不造 parallel transport。

#### 20. 风险与注意事项

主要风险是 prompt 注入、synthetic 伪造和 semantic/mechanical 污染；由 safe-display、synthetic、no-replacement tests 覆盖。

### Unit 3：接入 precheck gate 的 failure reason 与 check-unit guidance

#### 1. Unit 目标

普通 precheck rejection 按 failed_checks 注入 common + specific；synthetic rejection 只注入 common。

#### 2. 对应需求与 Scenario

R2、R3、R6；S1、S2、S4；D2、D3；E3-E5/E9-E10。

#### 3. 外部可观察结果

target producer 下一轮 prompt 同时看到 reason/failed_checks、common、实际失败 check 指导；retry/exhaustion 和 X.rejected schema 不变。

#### 4. 当前行为基线

handle_precheck_rejection 调 build_precheck_evidence；builder 不接受 rule guidance。现有 precheck scenario 已证明 target/synthetic behavior。

#### 5. 输入与输出

输入是 PrecheckRule.recovery_guidance 和 rejected failed_checks/synthetic。输出为 U2 normalized evidence。malformed payload 保持 no-invented-evidence；unknown check 只 common。

#### 6. 修改位置

- event_loop/precheck_gate_runner.rs::build_precheck_evidence：接受 rule/guidance，选择 specific；synthetic common-only。
- parse_and_emit/step_dispatch.rs::handle_precheck_rejection：把当前 rule guidance 传入同一 evidence/correction。
- ralph_config.rs::build_gate_instructions：仅在测试证明需要时追加 producer/gate 边界说明；不把 recovery guidance 复制给 gate。
- runner tests：numeric/string/malformed/synthetic/common/specific。

#### 7. 可依赖能力

U1 config、U2 renderer、RejectedPayload parser、existing target/retry。

#### 8. 禁止依赖的未来能力

不得改 consistency/CLI/red-team/skills/docs；不得读取 git/worktree。

#### 9. 验收测试

numeric/string failed check、multiple stable order、synthetic common-only、malformed no invented evidence、retry payload/exhaust unchanged；扩展 evidence_bound_precheck_routing.yml，保持 event expectations。

运行 cargo nextest run -p ralph-core -- precheck；cargo nextest run -p ralph-core --test scenarios -- precheck。

#### 10. Acceptance Red

先运行普通 rejection guidance test，当前 builder 不携带 guidance，应缺 common/specific；synthetic 误把 all prompt items 当 specific 应失败。时序 fixture 失败不算 Red。

#### 11. 单元测试拆分

numeric normalization、named/string identity、多 failed checks order/dedup、unknown/malformed generic-only、synthetic common-only、reason/retry/exhaust characterization。

#### 12. Red → Green → Refactor 顺序

numeric Red → rule-aware resolver → Green；string/multiple Red → normalization/order → Green；synthetic Red → common-only → Green；retry characterization → wiring → Green；Refactor；BDD regression。

#### 13. 最小实现范围

只绑定 rule guidance 到已有 evidence/correction；不改 gate decision、rejected schema、retry counter、workspace。

#### 14. 集成验证

真实 precheck desugar + gate + target producer；断言 prompt 和 accepted event sequence，不 stub gate。

#### 15. 风险驱动测试

Characterization + reject/resume/pass/exhaust 状态流；不加无依据 concurrency。

#### 16. 回归范围

precheck config/runner/desugar、precheck pass/exhaust BDD、correction、event_loop rejection。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| precheck_gate_runner.rs | 修改生产/测试 | selection | E4/E9 |
| step_dispatch.rs | 修改生产 | pass rule | E5 |
| evidence_bound_precheck_routing.yml | 修改 fixture | real routing | E10 |

#### 18. 完成标准

S1/S2 通过；reason/failed_checks/retry/exhaust characterization、target isolation、相关 nextest、Build/Lint/format 通过。

#### 19. 停止条件

failed_checks 类型与 E9 不符、synthetic 来源不稳定、或需要改 rejected schema 时，停止更新 D2/D3，不绕过 schema。

#### 20. 风险与注意事项

防止 specific 泄给 synthetic 或 gate hat；用 synthetic negative、target prompt inspection 和 runner tests 覆盖。

### Unit 4：接入 payload consistency、CLI policy-check 与 retry feedback

#### 1. Unit 目标

命中 PayloadConsistencyRule 时按稳定 rule id 注入 common/specific，并让 runtime correction 与 ralph emit --policy-check 使用同一 evidence/guidance 数据。

#### 2. 对应需求与 Scenario

R3、R4、R5、R6；S3、S4、S6；D1、D4；E6、E8、E11、E13。

#### 3. 外部可观察结果

命中 payload_consistency:rule-id 时，agent 看到 message/invariant、observed/proof、common 和当前 id specific；其它 rule 不泄漏；CLI JSON 与 prompt 不矛盾。

#### 4. 当前行为基线

validation.rs 为第一条命中 rule 构造 EvidenceDetail；policy.rs 已将 evidence 传到 correction；emit guide 已规定 semantic 不出现 suggested_*。

#### 5. 输入与输出

输入为当前 rule id 和 guidance。输出为 PolicyFinding.evidence、correction prompt、CLI JSON 的同源 guidance。未命中或无 specific 时只 common。只读当前 payload，保持 first-hit order。

#### 6. 修改位置

- event_policy/validation.rs consistency branch：调用 shared resolver，把 rule id 作为 selected key。
- event_policy/types.rs：仅按 U2 serde contract 调整 PolicyFinding/evidence 边界。
- event_loop/policy.rs：保持同一 evidence 到 prompt/JSON。
- CLI policy-check enrichment 的实际调用方：U4 先用 rg 确认真实文件/符号，再补同源 guidance serialization；不得编造路径。
- event_policy/tests/tests_part2.rs、event_loop/tests/event_policy.rs、evidence_bound_correction.yml：rule-id/no-hit/first-hit/no-replacement/real retry。

#### 7. 可依赖能力

U1 config、U2 renderer、U3 normalized evidence、现有 AST observation。

#### 8. 禁止依赖的未来能力

不得改 red-team、skills、docs；不得给 semantic 加 replacement command。

#### 9. 验收测试

rule-id exact match、wrong/other rule isolation、common-only fallback、non-hit no guidance、first-hit order、existing observed/invariant/proof、semantic no replacement、CLI/runtime same-source。

运行 cargo nextest run -p ralph-core -- event_policy payload_consistency correction；以及 U4 调查后确认的 ralph-cli integration_emit_policy 过滤器。U4 还必须确认 E19 所示 correction-only re-entry 在真实 target prompt 中仍成立。

#### 10. Acceptance Red

先运行 rule-id guidance 和 real scenario；当前 evidence 无 guidance，应缺失。CLI Red 必须针对 guidance missing，不得把已有 suggested_command omission 伪称为 Red。

#### 11. 单元测试拆分

rule-id match、wrong id、common fallback、non-hit、first-hit、evidence preservation、no replacement、CLI text/JSON same source。

#### 12. Red → Green → Refactor 顺序

selection Red → validation wiring → Green；non-hit/other-rule Red → explicit condition → Green；CLI same-source Red → evidence projection → Green；characterization → Refactor；real scenario regression。

#### 13. 最小实现范围

只改 consistency finding/evidence/correction/CLI projection；不改 predicate、policy decision、retry key、terminal。

#### 14. 集成验证

真实 validate_event_with_options、policy rejection、correction builder、CLI policy-check；确认 --policy-check 不写盘且 apply decision 一致。

#### 15. 风险驱动测试

first-hit/current-payload/no-replacement；不引入历史或跨事件测试。

#### 16. 回归范围

event policy part1/part2、payload evaluator、correction、CLI emit policy、evidence-bound BDD。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| event_policy/validation.rs | 修改生产/测试 | attach rule guidance | E8 |
| event_policy/types.rs | 按 contract 修改 | evidence projection | E6/E7 |
| event_loop/policy.rs | 修改生产/测试 | same correction | E6 |
| U4 调查确认的 CLI enrichment 文件 | 修改生产/测试 | same-source CLI | E13 |
| evidence_bound_correction.yml | 修改 fixture | real retry | E11 |

#### 18. 完成标准

S3/S6、CLI/runtime same-source、no replacement、first-hit/current payload/disabled regression、E19 correction-only re-entry、Build/Lint/format 全通过。

#### 19. 停止条件

若 CLI JSON 非 PolicyFinding.evidence 投影，先建立 shared normalization；若 semantic path已有 suggested_command，停止升级 D4；若 consistency reject 后真实 target 没有下一次 prompt，停止升级 D9，先确定统一 re-entry 入口。

#### 20. 风险与注意事项

防 runtime/CLI drift、rule-id 误配、guidance 进入 replacement 区；用 same-source、wrong-rule、CLI integration 和 no-replacement tests。

### Unit 5：用真实 EventLoop 场景固定完整 retry contract

#### 1. Unit 目标

证明两类 gate 都能完成拒绝 → target retry → guidance 可见 → 修复通过/耗尽，并固定无关 hat 不消费、synthetic 不伪造、old disabled path 不变。

#### 2. 对应需求与 Scenario

R2-R7；S1-S6；D1-D5；E9-E11/E18。

#### 3. 外部可观察结果

真实 scenario 的 event sequence、correction state、target prompt、accepted downstream 和 exhaustion terminal 全符合期望；相同 payload 不因 guidance 变假成功。

#### 4. 当前行为基线

已有两个 evidence-bound scenario 覆盖 target/synthetic/consistency retry，另有 correction_three_escalation 覆盖 escalation，但都没有 custom guidance。

#### 5. 输入与输出

输入只增加最小 guidance。输出增加 prompt presence/absence、evidence、target、retry count 断言；events/terminal/downstream 保持既有期望。

#### 6. 修改位置

- 两个 evidence-bound YAML：增加 guidance config/assertions，不改 topology/accepted semantics。
- correction_three_escalation.yml：只确认 guidance 不改 exhaustion。
- tests/scenarios.rs：仅在真实 runner 缺结构化 guidance assertion adapter 时扩展，不写 stub-only test。

#### 7. 可依赖能力

U1-U4 已验证的 config/evidence/renderer/policy。

#### 8. 禁止依赖的未来能力

不得模拟 runtime workspace snapshot/auto rollback；workspace 留 U6。

#### 9. 验收测试

precheck normal/synthetic、consistency reject/accept、same payload no false downstream、new evidence accepted/reset、exhausted terminal once、guidance omitted/default-off。运行 cargo nextest run -p ralph-core --test scenarios -- correction precheck payload_consistency。

#### 10. Acceptance Red

先运行扩展场景，当前 runtime 未写 guidance 时 guidance state 断言失败；若 event sequence 失败，先检查 fixture wiring，不放宽 expected events。

#### 11. 单元测试拆分

不复制 U1-U4 纯函数；只补 prompt visibility、queue consumption、events、retry reset/exhaustion、disabled no-op。

#### 12. Red → Green → Refactor 顺序

precheck Red → U3 Green；consistency Red → U4 Green；synthetic/no-replacement Red → U2-U4 Green；exhaustion/default-off characterization → fixture cleanup → subset regression。

#### 13. 最小实现范围

只改真实 scenario fixture/assertion；发现生产缺口必须回到拥有行为的 U1-U4。

#### 14. 集成验证

必须使用 run_workflow_guard_scenario/真实 EventLoop，断言 events 和 assert_state，禁止 run_scenario stub 作为唯一证明。

#### 15. 风险驱动测试

必要的 state-machine retry recovery；无依据不加 concurrency/fault injection。

#### 16. 回归范围

evidence-bound、precheck pass/exhaust、payload consistency accept/reject、correction escalation、chain_validation/event_policy 相关子集。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| 两个 evidence-bound YAML | 测试 fixture | BDD acceptance | E10/E11 |
| correction_three_escalation.yml | 必要时修改断言 | exhaustion regression | E18 |
| tests/scenarios.rs | 必要时修改 adapter | structured assertion | E10/E11 |

#### 18. 完成标准

场景全绿，无 skip/only/弱化断言，event/target/retry/terminal/disabled 全保留，Build/Lint/format 通过。

#### 19. 停止条件

runner 不能观察 guidance 而要求读内部 ledger 时，改用现有 prompt/assert_state adapter，不把内部路径注入 agent contract。

#### 20. 风险与注意事项

异步 correction 可能在同 iteration 不可见、无关 hat 可能先构建；沿用既有 target-aware 时序断言，不用延长 timeout 掩盖问题。

### Unit 6：为 red-team-attack 增加 workspace precheck 与专用 guidance

#### 1. Unit 目标

让 target-locker、plan-resolver、attack-surface-mapper、experiment-plan-validator、experiment-runner 在关键入口/出口保存 workspace 证据；生产树变更由现有 precheck gate 拒绝并 retry，下一轮得到 common/specific；runtime 不采集或自动修复。

#### 2. 对应需求与 Scenario

R8、R11；S7；D6、D8；E16。

#### 3. 外部可观察结果

red-team 仍只写 .ralph/red-team/**。五个高风险 hat 有 entry/exit precheck；非 .ralph 变更导致 rejection/retry；runtime 不执行 reset/stash/clean/delete。

#### 4. 当前行为基线

preset 已声明 read-only guardrail、部分 git status、precheck/consistency gate；事故证据显示 plan-resolver 产生过未跟踪源码文件。author notes 有 gate selection，但没有统一 workspace artifact/recovery contract。

#### 5. 输入与输出

输入是 entry HEAD/porcelain、允许根 .ralph/red-team/**、exit 状态和 guidance。输出是按现有 convention 命名的 workspace-precheck start/end artifact、failed_checks/reason、target retry prompt。tracked/modified/added/deleted/untracked 非允许路径失败；ownership 不明时停止，不 broad cleanup。

#### 6. 修改位置

- presets/en/red-team-attack.yml：五个 hat 加 entry/exit precheck、allowlist、recovery_guidance；保持 event topics/schema/topology。
- presets/en/red-team-attack-preset-author-notes.md：记录 workspace contract、guidance keys、retry/repair boundary、precheck gate 术语。
- presets/schemas/red-team-attack.yml：先比较；若只加 rule guidance 且 event fields 不变，保持 schema 不变并记录验证。
- presets/scenarios/red-team-attack-success.yml 或新增等价 verifier fixture：clean path 和 mutation/retry（若 verifier 能表达）。

#### 7. 可依赖能力

U3 precheck guidance、U4 consistency、U5 real scenario、现有 git status --porcelain=v1 --untracked-files=all instruction pattern。

#### 8. 禁止依赖的未来能力

不得实现 runtime snapshot、auto cleanup、通用 hash、跨 activation ownership inference；不得把 production mutation 变成合法实验副作用。

#### 9. 验收测试

- red-team strict lint 和 schema/event topology parity；
- success dynamic verify；
- mutation negative：tracked/untracked/allowed .ralph path、rejection/retry、prompt guidance、无 auto cleanup；
- 五个 hat 明确 entry/exit/allowed path/failure emit/retry repair/ownership stop。

运行 ralph preset check -H presets/en/red-team-attack.yml --strict --format json；ralph preset verify -H presets/en/red-team-attack.yml --scenario presets/scenarios/red-team-attack-success.yml --format json；新增 scenario 按同命令。

#### 10. Acceptance Red

先在真实 verifier/scenario 中断言当前 mutation path 不会产生统一 guidance/retry；不能只读 YAML 文本。success path 不得因可选 guidance 失败。

#### 11. 单元测试拆分

clean path、tracked mutation、untracked non-.ralph mutation、allowed .ralph artifact、unknown ownership stop、retry prompt guidance。不得新增 runtime worktree unit test。

#### 12. Red → Green → Refactor 顺序

mutation Red → entry/exit + gate guidance → Green；allowed path negative → allowlist clarification → Green；ownership stop Red → common/by_check prose → Green；success characterization → YAML/schema/comment refactor → lint/verify。

#### 13. 最小实现范围

只改 preset instructions、precheck rule guidance、author notes、真实 scenario；不改 Rust workspace behavior。agent 只能清理可证明属于本 activation 的 mutation。

#### 14. 集成验证

真实 preset normalize/lint/verify；external schema 做结构化 parity。无法表达 mutation 时必须记录 BLOCKED，不用 prompt contains git status 代替。

#### 15. 风险驱动测试

Characterization + tracked/untracked/allowed mutation fixture + ownership negative；不测试 broad git clean/reset。

#### 16. 回归范围

red-team strict lint、success verify、所有 redteam scenarios、schema parity、author notes gate audit；manifest/index/zsh 只确认名称未变。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| presets/en/red-team-attack.yml | 修改 preset | workspace precheck/guidance | E16/D8 |
| red-team-attack-preset-author-notes.md | 修改文档 | operator contract | E14/E16 |
| presets/schemas/red-team-attack.yml | 仅 parity 证明需要时修改 | schema SSOT | E16 |
| red-team success/negative scenario | 测试 fixture | dynamic behavior | E16 |

#### 18. 完成标准

五个 hat 纪律明确；mutation behavior 真实验证或明确阻塞；success/lint/schema parity 通过；无 runtime collector/auto repair。

#### 19. 停止条件

任一 hat 合法写生产树、artifact path 不可归属、schema drift、或 mutation 无法真实表达时，停止更新 D8/范围。

#### 20. 风险与注意事项

防 helper 误写 repo、.ralph 证据误报、retry broad cleanup；用 allowlist、start/end evidence、ownership stop、no auto cleanup 缓解。

### Unit 7：同步 author/review skill、agent-facing guide 与 human docs

#### 1. Unit 目标

让 preset author/reviewer 与 loop 内 agent 都知道新 guidance contract、official precheck gate、workspace precheck 和 retry 修复边界，并以 fixture/anchor 防漂移。

#### 2. 对应需求与 Scenario

R9、R10；S8、S9；D4-D8；E12-E15。

#### 3. 外部可观察结果

author 能声明/审核 recovery_guidance；review 独立检查 key、common/specific、semantic no-replacement、target/retry、synthetic、workspace precheck；agent guide 有可执行字段来源/动作/停止条件；human guide 与实际命令一致。

#### 4. 当前行为基线

两套 skill 已有 commands、finding-rubric、agent-native/prompt visibility、key-stage guard、evidence-bound findings；data docs 已有 semantic recovery 和 suggested_command 边界，但没有 custom guidance。

#### 5. 输入与输出

输入是 U1-U6 冻结的 YAML fields、finding IDs、prompt labels、red-team notes。输出是允许范围内的 skill references/SKILL/fixtures/tests、ralph-tools-precheck.md、recovery-directives.md、emit.md 和 human guides。

#### 6. 修改位置

- author references/SKILL：commands、finding-rubric、author-checklist、agent-native-model、prompt-visibility、patterns。
- review references/SKILL：同名 references 加 agent-skill-audit；保持 independent review。
- review fixtures/tests：positive/negative guidance、workspace、replacement command。
- data skill：precheck、recovery-directives、emit；写 trigger/action/source/stop，不写内部函数、ledger、计划编号、事故路径。
- human docs：precheck-gates、payload-consistency、preset-authoring；写 YAML 形状、lint、retry、synthetic、术语。

#### 7. 可依赖能力

U1 finding/field shape、U2 labels、U3/U4 behavior、U6 red-team contract。

#### 8. 禁止依赖的未来能力

不得宣称 runtime 自动采集/回滚 workspace、semantic suggested command config、未实现 CLI 参数；不得重新创建 ralph-preset-common。

#### 9. 验收测试

- author/review commands 都列 guidance shape 和真实 lint/verify commands；
- 两份 finding-rubric 都映射 runtime IDs 和 review-only guidance/workspace findings；
- positive fixture 无新 finding，negative fixture 命中预期；
- test_skill_anchors.py 通过；
- data docs 每条有 trigger/action/source/stop，scripts/check-cli-doc-drift.sh 通过；
- 需要命令语法时运行 ralph preset check --help、ralph emit --help。

#### 10. Acceptance Red

先在 skill negative fixture/anchor 中断言当前 references 不知道新 key/workspace contract，预期缺 anchor/finding。不能用 grep preset 文案作为唯一 Red。

#### 11. 单元测试拆分

author/review fields/commands parity、finding inventory/anchors、invalid key/unsafe text、missing common/specific、semantic replacement negative、workspace missing entry/exit/allowlist/ownership stop、forbidden internal terms、CLI doc drift。

#### 12. Red → Green → Refactor 顺序

anchor/fixture Red → references contract → Green；negative audit Red → fixture/rubric → Green；agent docs Red → data docs → Green；human docs/drift Red → docs → Green；Refactor duplicate prose但保持两套 references 独立。

#### 13. 最小实现范围

只同步已实现行为和审计规则；不在 skill 增加 runtime feature，不泄露 runtime internals，不用事故替代通用规则。

#### 14. 集成验证

运行 skill tests、preset review commands、CLI help、doc drift；对 red-team notes 做 author/review 双视角检查，reviewer 不盲信 notes。

#### 15. 风险驱动测试

negative fixture/anchor 和 forbidden-term audit；运行行为由 U5/U6 覆盖，无需额外 E2E。

#### 16. 回归范围

author/review Python tests、existing evidence-bound/key-stage fixtures、core/cli preset_lint、CLI help/doc drift、data skill review。

#### 17. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| author/review references/SKILL | 修改文档/规则 | operator contract | E14/E15 |
| review fixtures/test_skill_anchors.py | 修改测试 | audit contract | E15 |
| crates/ralph-core/data/ralph-tools-*.md | 修改 agent guide | recovery/policy fields | E13 |
| docs/guide/precheck-gates.md、payload-consistency.md、preset-authoring.md | 修改 human docs | author/operator usage | E12 |

#### 18. 完成标准

两套 skill 对称更新，fixtures/tests 通过，agent docs 满足注入规则，CLI/doc drift 通过，未新增 unsupported promise。

#### 19. 停止条件

runtime field/finding/CLI JSON 与 U1-U6 不一致时，回到拥有该 contract 的 Unit；如果需要不存在的 common skill，保持两套独立 references。

#### 20. 风险与注意事项

防 author/review 漏同步、agent guide 泄漏 internals、precheck_guard 被写成 runtime official name；通过对称 references、anchors、fixtures、forbidden-term、CLI drift 缓解。

## Unit 串行依赖图

U1 配置与 lint
  ↓ shared typed contract + finding IDs
U2 evidence/correction renderer
  ↓ normalized guidance + semantic safety
U3 precheck wiring
  ↓ precheck common/specific/synthetic behavior
U4 consistency + CLI wiring
  ↓ both gates share observable contract
U5 real EventLoop BDD
  ↓ behavior frozen
U6 red-team preset workspace precheck
  ↓ preset behavior frozen
U7 author/review skills + agent/human docs

- U1→U2：renderer 必须消费已验证 config contract。
- U2→U3：precheck 需要 safe renderer/semantic suppression。
- U3→U4：consistency 复用 precheck verified evidence shape。
- U4→U5：BDD 观察两类 runtime path，并确认 consistency 的 correction-only re-entry，不额外制造 task.resume。
- U5→U6：preset changes 依据真实字段/行为。
- U6→U7：skills/docs 描述最终 contract。
- 禁止提前实现：U1 不改 prompt；U2 不改 gate selection；U3 不改 consistency/CLI；U4 不改 preset；U5 不写 runtime workaround；U6 不加 runtime collector；U7 不加未实现行为。

## Verification Contract

### 9. 执行命令清单

所有命令按 Unit 串行执行；失败不得进入下一 Unit。

| 时机 | 命令 | 目的 | 预期 | 失败处理 |
|---|---|---|---|---|
| U1 | cargo nextest run -p ralph-core -- recovery_guidance precheck payload_consistency | config/lint unit | 全绿 | 确认真实 Red/修复 |
| U1 | cargo nextest run -p ralph-core -- preset_lint | lint regression | 全绿 | 停止 U1 |
| U2 | cargo nextest run -p ralph-core -- correction | renderer/serde/safety | 全绿 | 停止，不能弱化断言 |
| U3 | cargo nextest run -p ralph-core -- precheck | runner/desugar/retry | 全绿 | 停止 |
| U3 | cargo nextest run -p ralph-core --test scenarios -- precheck | real precheck | 场景通过 | 不放宽 events |
| U4 | cargo nextest run -p ralph-core -- event_policy payload_consistency correction | evaluator/correction/re-entry contract | 全绿 | 检查 same-source 和 target re-entry |
| U4 | cargo nextest run -p ralph-cli --test integration_emit_policy | CLI policy check（先确认实际入口） | 全绿 | 入口不符先更新证据/命令 |
| U5 | cargo nextest run -p ralph-core --test scenarios -- correction precheck payload_consistency | full retry/accept/exhaustion | 全绿 | 回到拥有行为的 Unit |
| U6 | ralph preset check -H presets/en/red-team-attack.yml --strict --format json | red-team lint/schema | 无 error | 修 preset/schema |
| U6 | ralph preset verify -H presets/en/red-team-attack.yml --scenario presets/scenarios/red-team-attack-success.yml --format json | success closure | pass | 修 fixture/preset |
| U7 | .venv/bin/python -m pytest skills/ralph-preset-review/tests | skill anchors/fixtures | pass | 保持对称 |
| U7 | scripts/check-cli-doc-drift.sh | docs/source refs | exit 0 | 修 docs |
| 每 Unit | cargo fmt --check | format | exit 0 | 只修本 Unit |
| 每 Rust Unit | cargo build；cargo clippy | build/typecheck/lint | exit 0 | 修真实错误 |
| 最终 | ./scripts/run-tests.sh | full nextest phases | 全绿 | 仅按 AGENTS 规定处理竞态 fallback |

命令语法：若 U4 调查确认实际 CLI integration test 路径/过滤器不同，Executor 必须用代码证据替换命令并记录；不能猜或跳过。禁止裸 cargo test -p ralph-cli。

### 10. 最终质量门禁

- R1-R11 每项关联 Scenario、可执行测试、Evidence。
- U1-U7 严格串行，每个 Unit 有真实 Red、最小 Green、Refactor、Integration、Regression、Close。
- 两类 gate 都把失败原因、common、specific 或 synthetic common-only 送到正确 target；consistency correction-only re-entry 已由真实 scenario 证明，不额外产生重复 task.resume。
- semantic rejection 不提供 replacement payload/成功值/suggested_command。
- task.resume routing、target isolation、retry counter/reset、exhaustion 与旧行为保持。
- 无 guidance/disabled 旧 preset 无回归。
- unsafe/oversized/unknown guidance strict lint fail-close，IDs 已进入 ALL_FINDING_IDS 和两套 rubric。
- red-team 五个高风险 hat 只写 .ralph/red-team evidence；越界变更 rejection/retry；runtime 无 workspace collector/auto repair。
- red-team schema/topology/success verify 通过；manifest/index/zsh 仅确认名称未变。
- agent docs 每条有 trigger/action/source/stop，无内部函数/ledger/事故泄露。
- author/review 独立、对称，不创建 common skill。
- fmt、build、clippy、相关 nextest、Python tests、doc drift、最终 run-tests 全通过。
- 无 skip/ignored/only、无弱化断言、无无解释 snapshot/golden、无 BLOCKED decision、无计划外文件。

## Definition of Done

### 全局完成条件

只有 U1→U7 每个 Unit 满足完成标准、所有 Decision confidence 仍 ≥0.85、最终质量门禁全部通过，计划才算完成。实现中若发现代码与证据冲突，必须记录新证据、更新影响分析、重比方案、重新决策并修订当前/后续 Unit，不能临时拍板。

### 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每个 Unit 指向真实入口、输入/输出、Red、测试、回归和停止条件 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D9 已选择方案；仅保留有验证动作的事实确认 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E19；新增文件标为新增并有 U1 验证 |
| 所有关键决策置信度是否 ≥0.85 | 是 | D1-D9 为 0.88-0.98 |
| 是否存在未处理的低置信度假设 | 否 | CLI projection/artifact path 已登记验证，不允许猜测 |
| 每个 Unit 是否只有一个可观察行为 | 是 | config/lint、render、precheck、consistency、BDD、preset、docs/skills 分开 |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 有 tests/commands/完成标准 |
| 每个 Unit 是否有真实 Red | 是 | Red 均来自目标能力缺失；环境/fixture/命令失败不算 |
| 每个 Unit 是否包含回归范围 | 是 | 每个 Unit 列 direct/adjacent/full affected tests |
| 是否存在未来 Unit 依赖 | 否 | 依赖只沿 U1→U7 |
| 是否存在泛化任务描述 | 否 | 动作绑定文件、符号、输入、断言、停止条件 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | S1-S9 → R matrix → U1-U7 |
| 所有关键决策是否有 Evidence | 是 | D1-D9 引用 E IDs |
| 计划是否可以严格串行执行 | 是 | 串行图和逐 Unit Close gate 已定义 |

## Appendix：Executor 必须遵守的决策摘要

1. 只扩展现有 evidence-bound correction；不得新建 recovery topic 或 parallel resume channel。
2. 配置字段固定为 recovery_guidance.common 与 recovery_guidance.by_check；precheck key 是 1-based checklist index，consistency key 是 rule id。
3. 普通 precheck rejection 选择 failed_checks 对应 specific；synthetic 只显示 common。
4. semantic guidance 是修复指导，不是 replacement payload/command；最终仍先 policy-check，再正式 emit。
5. red-team workspace precheck 由高风险 hat 自己执行并写 .ralph/red-team/**；runtime 不采集、不 reset、不 stash、不 clean、不自动删除。
6. precheck_guard 继续是 author notes 选择字段；面向用户的 runtime 能力名称统一写 precheck gate。
7. 任何代码、测试、CLI 入口与本计划证据冲突时停止当前 Unit，不得为了保持计划完整而猜测。

## Appendix：U6 mutation 真实触发限制 BLOCKED（plan 2026-08-17-1841 fix-plan §19 fallback）

`presets/scenarios/*.yml` 为 Mock-based scenario，verification fixture 不写入 production tree mutation；workspace precheck 真实触发链路（tracked test mutation、untracked non-.ralph mutation、allowed .ralph artifact、unknown ownership stop）需 future plan 设计 red-team verifier mutation negative fixture（per G3）。当前 plan 已完成：

- 5 hat 各自 `instructions:` 显式包含 entry/exit evidence 写入步骤（`.ralph/red-team/<NN>-workspace-<hat>.md` + `git status --porcelain=v1 --untracked-files=all` 快照）。
- `recovery_guidance` 5 hat 各自 1 条 precheck rule（target-locker / plan-resolver / attack-surface-mapper / experiment-plan-validator（plan.valid + plan.invalid）/ experiment-runner），`on_exhausted: redteam.failed(failure_kind=workspace_precheck_failed)`。
- 既有 `git status --porcelain=v1` 三处全部改为 `--porcelain=v1 --untracked-files=all`（per S3）。

未来 plan 入口：mutation negative fixture 设计（建议 plan 标题前缀 `2026-MM-DD-NNN-feat-red-team-mutation-negative-fixture-plan`）。
