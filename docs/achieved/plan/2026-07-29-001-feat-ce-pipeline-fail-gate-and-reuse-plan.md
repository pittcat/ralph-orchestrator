---
title: ce-executor-pipeline 阈值化 Fail 门禁与 Rerun 复用增强计划
type: feat
date: 2026-07-29
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan
execution: code
---

# ce-executor-pipeline 阈值化 Fail 门禁与 Rerun 复用增强计划

## 0. 计划状态

**READY，首版。**

- **代码基线：** `445d7610`（`fix(preset): parallel-forge executor 接入 startup_grace_secs 冷启动宽限`）。
- **问题来源：** operator 实测反馈——① executor/fixer 太容易在证据不足时 emit fail（`work.failed` / `fix_status: blocked`）直接短路到 reporter；② reporter 后 rerun 时旧 worktree 不干净、上一轮经验借鉴不足。
- **调查范围：** `presets/en/ce-executor-pipeline.yml`（全量）、`crates/ralph-core/src/config/precheck.rs`、`crates/ralph-core/src/config/ralph_config.rs`（desugar）、`crates/ralph-core/src/event_policy_payload_consistency.rs`、`crates/ralph-core/src/event_policy.rs`（`.proposed` 处理）、`crates/ralph-core/src/event_loop/{mod.rs,precheck_gate_enforcement.rs,precheck_gate_runner.rs}`、`crates/ralph-core/src/worktree.rs`（复用归档）、`crates/ralph-cli/src/commands/run.rs`（reuse 入口）、`crates/ralph-core/data/ralph-tools-precheck.md`、`crates/ralph-core/tests/scenarios/{2026-07-02-precheck-gate-pass,2026-07-02-precheck-gate-exhaust,ce_executor_pipeline_executor_fail_stop}.yml`、`presets/index.json`、`presets/templates/{parallel-forge,red-team-attack}/`、`crates/ralph-cli/src/builtin_artifact_templates.rs`、`crates/ralph-cli/build.rs`（模板拷贝）、`presets/en/parallel-forge.yml`（materialize 用法）。
- **已执行验证：** precheck 脱糖/合成 gate/on_fail 路由/on_exhausted payload 全链源码勘察；payload_consistency 操作符集确认；BDD 场景独立性确认（自带 config，不受 preset schema 改动直接击穿）；worktree 归档内容清单确认；模板机制三处接线点（templates 目录 + `builtin_artifact_templates.rs` 注册 + `build.rs` 拷贝）与 hat 引用形态（`ralph preset materialize-artifacts <preset> --plan-key <key>` → 复制填写）确认。
- **尚未执行验证：** 各 Unit 的 Acceptance Red/Green、`ralph preset check`、`preset_lint`（含 EGRESS_MAX_HOPS 跳数复核）、BDD 新场景、全量测试，由对应 Unit 执行。
- **阻塞项：** 无。

---

## 1. 功能目标

### 业务目标

1. **Fail 必须有证据（阈值化 fail 门禁）**：executor 的 `work.failed` 与 fixer 的 `fix.done{fix_status: blocked|partial}` 从「agent 自觉的软规则」升级为「独立 LLM gate 他评 + 确定性规则机械门」的双层硬门禁。证据不足时 gate 打回 producer 继续干活；连续 3 次打回仍不合格的 fail 才是诚实终局。
2. **Rerun 复用增强**：rerun 时 plan-reviewer 以「上一轮 report 优先」重建经验；fixer 能读到归档的失败教训；与剩余 Unit 无交集的残留脏文件不再一票否决 rerun。

### 用户或调用方

- `ce-executor-pipeline` 的 operator：fail 结论可信（被独立 agent 审过）；rerun 不再被无关残留卡死；每轮经验显式沉淀进下一轮。
- preset 评审者：`payload_consistency` 机械门与 `event_loop.precheck` LLM 门的分工范式在本 preset 首次成对落地，可作后续 preset 参考。

### 当前行为（均有证据）

1. **抗 fail 软规则无 runtime 强制**：executor/fixer instructions 已有 per-Unit 3-retry、不许早退、`work.failed` 限零交付死胡同（yml:2029-2071、4445-4474），但全部停留在 prompt 文本层。现有 3 条 `payload_consistency` 规则（yml:121-146）只管 green/red 矛盾；`work.failed` 带非空 `completed_units`、`fix_status:blocked` 带非空 `completed_fix_units` 等违禁形状 runtime 照单全收。
2. **fail 不需要证据**：emit `work.failed` 无需落盘任何 retry 证据文件，无需自评置信度；agent 「觉得解决不了」即可认输短路到 reporter（reporter triggers 含 `work.failed`，yml:4908）。
3. **归档摘要太薄**：`--reuse-worktree` 归档 `.ralph/` runtime 文件到 `reuse-history/<ts>/` 后写的 `resume-context.md` 是固定模板（worktree.rs:875-884）——只有归档路径 + 三句通用指导；上一轮 report.md / verdict / completed units / 残留全部要 plan-reviewer 自己从归档重新挖掘。
4. **经验链到 executor 为止**：`reuse_guidance_file` 只挂 `plan.ready` payload；fixer 的 re-entry reconciliation（yml:4589-4598）只读当前 `.ralph/agent/decisions.md`——rerun 后那是空的新文件，旧 `fixer checkpoint:` / failure briefs 在归档里，fixer 不知道该去读。
5. **脏 worktree 处置极端**：git 工作树不归档不清理；executor Step 1.25（yml:2279-2289）对不可归因脏文件只能 `work.failed{unattributable_dirty_worktree}`，无「与剩余 Unit 无交集则隔离记录继续」的降级路径。

### 目标行为与差异

- **双层 fail 门禁**：
  - 第一层（确定性规则，`event_policy`）：`work.failed` + `completed_units` 非空 → reject；`fix_status:blocked` + `completed_fix_units` 非空 → reject（把已有软规则硬化，零误伤）。
  - 第二层（LLM gate，`event_loop.precheck`）：`work.failed` / `fix.done` 各挂一条 precheck rule。producer emit 后脱糖落 `<X>.proposed` → 合成 gate hat 激活一轮，按 checklist **打开证据文件实质审核**（自评变他评）→ 过则原样转发 `<X>`，不过则 `<X>.rejected` 带可执行 `failed_checks` → runtime 自动 `task.resume` 打回 producer（retry_budget=3）→ 预算耗尽 runtime 直发 `plan.blocked{kind: precheck_exhausted}` → reporter 走新增的 precheck_exhausted 分支收尾。
- **阈值化自评 rubric（自评/他评同源，模板单点维护）**：rubric（confidence 四维度评分对照表 + 阈值表 + coverage 计算规则 + 六类 failed_checks 定义）抽到 `presets/templates/ce-executor-pipeline/fail-confidence-rubric.template.md`，经 `ralph preset materialize-artifacts` 运行时落盘；executor/fixer instructions 与 gate checklist **都引用同一份模板**（自评=他评同一标准，消除两处同文漂移）。executor/fixer fail 前必须按 `settlement-evidence.template.md` 写证据文件、按 rubric 自评 `confidence` 与 `evidence_coverage`；confidence ≥90 才允许 fail，<90 由 gate 打回。
- **schema 留痕**：`work.failed` required_fields += `dead_end_confidence` / `dead_end_evidence_coverage` / `dead_end_evidence_file`；`fix.done` required_fields += `settlement_confidence` / `settlement_evidence_coverage` / `settlement_evidence_file`（events.jsonl 留数字，事后可分析）。
- **rerun 复用**：plan-reviewer Step 2.25 改 report-first（优先读归档 `report.md` + `report-input.*.json`），reuse-guidance.md 新增 `Last run terminal state` 节；fixer re-entry 经 `resume-context.md` 读归档 decisions.md；executor Step 1.25 新增 `foreign_dirt` 降级（与全部剩余 Unit 的 allowed files 无交集的脏文件：记录、不动、继续执行）。

### 输入 / 输出 / 状态变化 / 错误语义

- **输入：** preset YAML 的 `event_loop.precheck.rules`（新增块）、`event_policy.schemas` 新 required 字段、`payload_consistency` 新规则；executor/fixer/plan-reviewer instructions 文本。
- **输出：** `work.failed` / `fix.done` payload 新增置信度与证据字段；gate 产出的 `<X>.rejected{failed_checks, reason}`；gate 放行原样转发的 `<X>`；on_exhausted 的 `plan.blocked{topic, reason, kind: precheck_exhausted}`。
- **状态变化：** 拓扑 14 hat → 16 hat（合成 `precheck-work.failed`、`precheck-fix.done` 两个 gate hat）；executor/fixer 的 `publishes`/`terminal_events` 中 `work.failed`/`fix.done` 被脱糖改写为 `.proposed` 变体（runtime 行为，preset 文本不变）。
- **错误语义：**
  - 形状违禁（completed 非空仍 fail）→ `payload_consistency:<rule_id>` reject，message 指引改 emit partial/done。
  - 证据不足 → gate `<X>.rejected{failed_checks}`，`task.resume` 打回，producer 按缺口继续。
  - 3 次打回仍不合格 → runtime 直发 `plan.blocked{kind: precheck_exhausted}`（绕过 schema，见 E8），reporter 以 verdict=blocked 收尾。
  - `RALPH_PRECHECK_MODE=off` → 不脱糖不跑 gate，行为回退到现状（紧急关停阀）。

### 兼容、性能、安全与约束

- **兼容：** `event_loop.precheck` 为既有 opt-in 能力（2026-07-02-004 plan），本计划无 runtime 行为改动。runtime 触点仅两处**模板注册接线**（`builtin_artifact_templates.rs` 增加 ce-executor-pipeline 分支、`build.rs` 增加拷贝调用——纯注册，不改变任何运行时行为）外加一个条件性候选（EGRESS_MAX_HOPS 跳数复核结果若超 12 则需调常量，见 KTD8/U3 核对点）。旧 YAML 不含 precheck 块时行为逐位不变。BDD 旧场景自带独立 config，不受 preset schema 新字段影响。
- **性能：** 每次 `work.failed` / `fix.done` 多一轮 gate hat activation；fail 本应是低频事件，成本可忽略。`fix.done` 每次都会经过 gate（applied 直接放行），一轮只读 payload 的轻审核。
- **安全：** 不放宽 EventOriginGuard / topic_deny_rules 任何边界——deny 规则对派生 topic 天然安全（未声明即拒），gate hat 不在任何 deny 列表故可发真 `<X>`（E5）。gate hat 只读 evidence 文件做判断，不修改代码。
- **测试入口：** 一律 `cargo nextest run` 系列（AGENTS.md HARD RULE 1/2）；BDD 用 `run_workflow_guard_scenario`（真 EventLoop runner，禁止 `run_scenario` stub）。
- **Preset 文本约束：** 不新增「校验 preset 文本是否包含某段文字」的测试（AGENTS.md Preset 测试规则）；覆盖结构化语义（`RalphConfig::parse_yaml`、schemas、required_fields、lint findings、BDD）。

### 本次范围

- `presets/templates/ce-executor-pipeline/`（新增）：`fail-confidence-rubric.template.md`（rubric 对照表：四维度评分 + 阈值 + coverage 规则 + 六类 failed_checks 定义）、`settlement-evidence.template.md`（executor/fixer 共用的证据文件格式模板）、`README.md`（对齐既有模板目录惯例）。
- `crates/ralph-cli/src/builtin_artifact_templates.rs`：`CE_EXECUTOR_PIPELINE_TEMPLATE_NAMES` 常量 + include_str 模板数组 + `templates_for_preset` 匹配分支 + 测试。
- `crates/ralph-cli/build.rs`：`copy_preset_templates(..., "ce-executor-pipeline", ...)` 拷贝调用。
- `presets/en/ce-executor-pipeline.yml`：`event_policy.schemas`（2 个 topic 新字段+field_docs）、`payload_consistency`（2 条新规则）、`event_loop.precheck`（新块 2 条 rules）、executor / fixer / plan-reviewer / reporter 四段 instructions 升级（rubric 段改为 materialize→读模板→按模板自评/审核）、头部拓扑注释同步（14→16 hat）。
- `crates/ralph-core/tests/scenarios/`：新增 2 个 fail-gate 场景（rejected→pass、exhaust）。
- `CLAUDE.md` + `AGENTS.md`：builtin preset 描述行同步（HARD RULE 同步要求）。
- 条件性改动（仅当 U3 复核确认超跳）：`crates/ralph-core/src/preset_lint/workflow_activation.rs` 的 `EGRESS_MAX_HOPS` 常量 12→13。

### 非目标

- 不改 runtime 归档机制（worktree.rs 不生成 machine-readable archive-summary——留作后续候选）。
- 不改线性拓扑（不加 review/fix 回路——那是 `ce-executor-pipeline-loop` 的领域）。
- 不动 per-Unit retry 次数（3 次）与 hat timeout 值。
- 不对 `evidence_coverage` 设 runtime 数字门槛（第一版仅 gate 审核项；观察谎报情况后再议）。
- 不对 `work.done{execution_status: partial}` 加门禁——它是链路内结算（后续 stabilizer/review/fix 继续收敛），非终局认输。
- 不改 `ce-executor-pipeline-loop` / 其他 preset（含 parallel-forge / red-team-attack 的既有模板）。
- 不扩展 `payload_consistency` 操作符（`lt` 等）——阈值判断归 LLM gate，确定性层只做形状门禁。
- 不改 `presets/manifest.yml` / `presets/schemas/`（本 preset 无 SSOT schema 文件、无增删 preset）；zsh 补全不涉及。
- 模板机制不新增专用别名命令（如 `materialize:forge` 那种）——只用通用 `ralph preset materialize-artifacts ce-executor-pipeline --plan-key <key>`。

### 已确认假设 / 待验证假设

- **已确认：** precheck 脱糖只改写 `publishes`/`terminal_events`（ralph_config.rs:171-192），合成 gate hat 带 `max_activations = retry_budget+1`（ralph_config.rs:196-211）；event_policy 对 `.proposed` 有原生 dedup/prune 处理（event_policy.rs:784-794,1399-1411），无需手动加 `business_topics`；rejected 打回走 `task.resume` + correction 注入（mod.rs:12880-12939）；BDD 场景自带独立 config（E10）。
- **待验证（Unit 内验证）：** EGRESS_MAX_HOPS=12 是否覆盖加 gate 后的最长链（U3 跑 lint 确认）；`exempt_topics` 与脱糖后 emit 映射的交互（U3 核对：executor `exempt_topics: [work.done, work.failed]` 在 publishes 改写为 `.proposed` 后是否仍豁免）；gate hat 转发 payload 时 required_fields 校验路径（gate emit 真 `<X>` 走 validate_event，payload 为原样转发故字段齐全——U3 用 BDD 验证）。

---

## 2. 代码库现状与证据

### 2.1 当前实现入口

- **fail 软规则（无强制）：** executor yml:2029-2071（anti-abdication + finish-the-plan + 3-retry）、yml:2674-2698（Failure Path）；fixer yml:4445-4479（同构）；executor Confidence Protocol yml:2700-2704（0-100 软框架，无阈值无强制）。
- **确定性门禁现状：** `payload_consistency` 3 条规则 yml:121-146；操作符集 `eq/ne/gt/gte/exists/non_empty`（event_policy_payload_consistency.rs:77，无 `lt`）；命中即 `SemanticGateViolation`（event_policy.rs:2346-2378）。
- **precheck 机制（既有能力）：** 配置 schema `config/precheck.rs:15-58`（`rules.<X>.prompt: Vec<String>`、`on_fail{target, retry_budget=3, on_exhausted, reason}`）；脱糖 `ralph_config.rs:158-215`（改写 producer + 合成 gate hat + `inject_precheck_event_schemas`）；gate 执行与 enforcement `event_loop/precheck_gate_enforcement.rs`；打回路由 `event_loop/mod.rs:12855-12958`（`task.resume` + correction / 耗尽直发终态）；on_exhausted payload `event_loop/precheck_gate_runner.rs:273-280`（**仅 `{topic, reason, kind: precheck_exhausted}` 三字段**）；agent 行为 skill `crates/ralph-core/data/ralph-tools-precheck.md`。
- **reporter 消费面：** triggers 含 `work.failed` / `plan.blocked`（yml:4908）；plan.blocked 的 report_input_file 分支（yml:4922 起）。
- **reuse 机制：** 归档 `worktree.rs:748-869`（`.ralph/review/` 整树、decisions.md、summary.md、handoff.md 等移入 `reuse-history/<ts>/`）；`resume-context.md` 模板 `worktree.rs:875-884`；plan-reviewer Step 2.25 yml:1710-1747；executor Step 1.25 yml:2248-2299；fixer re-entry yml:4589-4598。
- **WAC 跳数：** `EGRESS_MAX_HOPS = 12`（workflow_activation.rs:465）；现链 `work.done → … → report.done` 恰为 12 跳（头部注释 yml:51-54）。
- **模板机制（既有能力）：** `presets/templates/{parallel-forge,red-team-attack}/` 两例；注册三件套——NAMES 常量 + include_str 数组 + `templates_for_preset` 匹配分支（builtin_artifact_templates.rs:43-145,183-189）、`build.rs:199-249` `copy_preset_templates` 拷贝；hat 引用形态 `ralph preset materialize-artifacts <preset> --plan-key <key>` 落盘后复制填写（parallel-forge.yml:303-306、guardrail yml:246）；既有测试模式 builtin_artifact_templates.rs:258-340（NAMES 校验 / 未知 preset 报错 / 幂等）。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | yml:121-146 + event_policy_payload_consistency.rs:77 | 现有 3 条 consistency 规则只管 green/red；操作符无 `lt` | 阈值判断不能靠确定性层 → 归 LLM gate；确定性层只做形状门禁 | 高 |
| E2 | ralph_config.rs:158-215 | 脱糖只改 `publishes`/`terminal_events`；合成 gate hat（Default HatConfig + 渲染 checklist + max_activations=budget+1） | `exempt_topics`/`topic_deny_rules`/`business_topics` 需逐一核对（U3 核对点） | 高 |
| E3 | event_policy.rs:784-794, 1399-1411 | `.proposed` 有原生 dedup + gate 放行时 prune | 派生 topic 无需手动进 `business_topics` | 高 |
| E4 | mod.rs:12880-12939 | rejected → correction + `task.resume(target)` + redispatch obligation | 打回循环原生；executor/fixer instructions 需写 Rejected-Resume 纪律 | 高 |
| E5 | yml:152-400（deny rules）+ ralph_config.rs:196-211 | deny 列表是手写 12 hat；gate hat 不在其中 | gate hat 发真 `<X>` 不被 deny；派生 topic 对其他 hat「未声明即拒」，无需补 deny 行 | 高 |
| E6 | precheck_gate_runner.rs:273-280 + mod.rs:12949-12954 | on_exhausted 的 `plan.blocked` payload 仅 `{topic, reason, kind}`，runtime 直发绕过 schema | reporter 必须新增 `kind=precheck_exhausted` 分支（无 plan_name / report_input_file 时生成简版 blocked 报告） | 高 |
| E7 | worktree.rs:748-884 | 归档含 review/ 整树 + decisions.md；`resume-context.md` 仅固定模板 | report-first 经验挖掘有物可依（归档 report.md 存在）；fixer 可经 resume-context 找到归档 decisions.md | 高 |
| E8 | ce_executor_pipeline_executor_fail_stop.yml:25-80 等 | BDD 场景自带独立 hats/event_loop/event_policy config | preset schema 新字段不击穿旧场景；新场景照 `2026-07-02-precheck-gate-*.yml` 模板写 | 高 |
| E9 | workflow_activation.rs:465 + yml:51-54 | EGRESS_MAX_HOPS=12；现最长链 work.done→report.done 恰 12 跳；fix.done 加 gate 后 +1 跳 | 大概率需 12→13（放宽只增通过面，不回破其他 preset；U2 以 lint 实跑为准） | 中 |
| E10 | ralph-tools-precheck.md:69-71, 95-101 | producer 照常 `ralph emit <X>`，解析层落 `.proposed`；被打回后读 `failed_checks` 再 emit 同一 topic | executor/fixer instructions 的 emit 示例无需改写为 `.proposed`（脱糖透明）；需补「被打回后行为」与 skill 引用 | 高 |
| E11 | yml:679-741（work.failed schema）、1286-1453（fix.done schema） | 两 topic 现有 required_fields 清单 | 新字段逐个加 required + field_docs（meaning/source/fill_rule） | 高 |
| E12 | yml:1710-1747（Step 2.25）、4589-4598（fixer re-entry）、2248-2299（Step 1.25） | 三段 reuse 相关 instructions 现状 | 三处改造锚点 | 高 |
| E13 | builtin_artifact_templates.rs:43-145,183-189 + build.rs:199-249 | 模板注册三件套（NAMES 常量 + include_str 数组 + 匹配分支 + build.rs 拷贝）为纯声明式扩展 | 新 preset 模板接线是机械注册，无 runtime 行为风险；测试模式现成 | 高 |
| E14 | parallel-forge.yml:303-306,246 + red-team-attack 模板目录 | hat 以「materialize → 从落盘 templates 目录读/复制」形态引用模板；目录含 README.md 惯例 | ce-executor-pipeline 模板同构；executor/fixer/gate 三方引用同一 rubric 模板可行 | 高 |

### 2.3 受影响范围

| 文件 | 改动 | Unit |
|---|---|---|
| `presets/templates/ce-executor-pipeline/fail-confidence-rubric.template.md` | 新增（rubric 单点维护） | U2 |
| `presets/templates/ce-executor-pipeline/settlement-evidence.template.md` | 新增（证据文件格式） | U2 |
| `presets/templates/ce-executor-pipeline/README.md` | 新增（目录惯例） | U2 |
| `crates/ralph-cli/src/builtin_artifact_templates.rs` | NAMES + 数组 + 匹配分支 + 测试 | U2 |
| `crates/ralph-cli/build.rs` | copy_preset_templates 调用 | U2 |
| `presets/en/ce-executor-pipeline.yml` | schemas/payload_consistency/precheck 块/4 段 instructions/头注释 | U1, U3-U6 |
| `crates/ralph-core/tests/scenarios/ce_executor_pipeline_fail_gate_rejected_then_pass.yml` | 新增 | U7 |
| `crates/ralph-core/tests/scenarios/ce_executor_pipeline_fail_gate_exhaust.yml` | 新增 | U7 |
| `crates/ralph-core/tests/scenarios.rs` | 注册 2 个场景函数 | U7 |
| `CLAUDE.md` + `AGENTS.md` | builtin preset 描述行同步 | U7 |
| `presets/index.json` | 描述行行为同步（可选，不含结构化字段） | U7 |
| `crates/ralph-core/src/preset_lint/workflow_activation.rs` | 条件性：EGRESS_MAX_HOPS 12→13 | U3（仅当 lint 确认超跳） |

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| KTD1 | 阈值校验放哪一层 | 扩展 `payload_consistency` 加 `lt` 操作符（runtime 数字校验）；`event_loop.precheck` LLM gate 他评 | precheck LLM gate | E1/E4/E10；confidence 是否被证据支撑是主观判断，确定性规则永远查不了「证据是否真的够」 | `lt` 版 runtime 也只能验证自报数字 ≥90，防造假能力与 gate 等价但无实质审核；且触发 runtime 改动 + lint + skill 同步链 | 0.90 |
| KTD2 | 确定性层还做什么 | 什么都不加；加形状门禁 | 加 2 条形状门禁（completed 非空不许 fail） | E1；ralph-tools-precheck.md:97「机械检查归 event_policy」 | 不加则「completed 非空仍 emit work.failed」这类纯机械违禁也要等 LLM gate 一轮，浪费且职责错位 | 0.92 |
| KTD3 | 置信度字段形态 | 布尔申报（`dead_end_gate: passed`）；数字字段照填（confidence/coverage） | 数字字段照填 + gate 审核 | 用户明示数字框架（90/75 阈值）；events.jsonl 留痕可分析谎报分布 | 布尔申报无数字留痕、审计价值低；防造假能力两者等价（约束来自证据文件 + gate 他评） | 0.85 |
| KTD11 | rubric 载体 | instructions 与 gate checklist 两处同文；`presets/templates/` 模板单点维护 | **模板单点维护**：`fail-confidence-rubric.template.md` 经 materialize 落盘，executor/fixer instructions 与 gate checklist 三方引用同一份 | E13/E14；operator 明确要求模板化；两处同文违反 AGENTS.md「引用不复述」防漂移原则，模板机制是既有能力 | 两处同文：rubric 迭代时三处手工同步必然漂移，gate 与 producer 标准失配后门禁失效 | 0.90 |
| KTD4 | fix.done 的 gate 范围 | 全量审核；按 fix_status 分支 | `applied` 直接放行，`partial`/`blocked` 实质审核 | fixer applied 是高频正常路径；fail 门禁的目标是「认输形状」 | 全量审核给正常路径加无谓成本且无收益 | 0.90 |
| KTD5 | coverage 是否设 runtime 门槛 | payload_consistency `gte` 规则；仅 gate 审核项 | 仅 gate 审核项（≥75 合格，<75 须有缺口解释） | coverage 的「可复核性」判断本身是主观的（gate 要打开来源验证） | 确定性层只能查数字 ≥75，查不了来源真假；KTD1 同理 | 0.80 |
| KTD6 | work.done partial 是否加门禁 | 加；不加 | 不加 | work.done partial 继续走 stabilizer/review/fix 链，非终局认输；用户痛点是「fail 直接到 reporter」 | 给链路内结算加门禁会逼 executor 对「U2 修不了」也举证 90%，与 stabilizer/fix 的收敛职责重复 | 0.82 |
| KTD7 | on_exhausted 终态形状 | 默认 `plan.blocked(reason=precheck_failed)`；定制其他 topic | 默认 + reporter 新增 `kind=precheck_exhausted` 分支 | E6：payload 仅三字段、runtime 直发绕过 schema；reporter 已消费 plan.blocked | 定制 topic 要新增路由与 schema，违背最小改动；分支是 instructions 层的事 | 0.85 |
| KTD8 | EGRESS_MAX_HOPS 处理 | 调 12→13；拓扑豁免；不改 | U2 实跑 lint 后定：超跳则调常量（放宽只增通过面） | E9 | 豁免机制目前不存在于 WAC；盲改常量无证据 | 0.75 |
| KTD9 | rerun 经验挖掘优先级 | resume-context 模板增强（改 runtime）；report-first（preset 文本） | report-first 纯 preset 文本 | E7：归档 review/ 整树含 report.md + report-input.*.json，是最浓缩的上轮结论 | 改 runtime（worktree.rs 生成摘要）跨 crate 且用户选定纯 preset 路径 | 0.88 |
| KTD10 | 不可归因脏文件处置 | 维持 `work.failed{unattributable_dirty_worktree}`；foreign_dirt 降级 | 与全部剩余 Unit allowed files 无交集 → `foreign_dirt` 记录继续；有交集且冲突 → 维持 fail | 用户确认；现状把「上轮合法 dirty 路径残留」升级成整轮 fail | 维持现状会让 rerun 被无关残留一票否决，与复用机制目标冲突 | 0.85 |

---

## 4. BDD 行为规格

```gherkin
Feature: F1 阈值化 fail 门禁（双层）

  Background:
    Given preset 声明 event_loop.precheck.rules[work.failed, fix.done]
    And payload_consistency 新增 2 条形状规则
    And work.failed schema 新增 3 个 required 字段
    And fix.done schema 新增 3 个 required 字段

  Scenario S1: work.failed 证据充分 gate 放行
    Given executor 的 dead-end-evidence.md 含每个 failed Unit 的 4 次尝试、≥3 角度、完整因果链
    And payload dead_end_confidence=92, dead_end_evidence_coverage=80
    When executor emit work.failed
    Then 落盘 work.failed.proposed
    And gate hat 审核通过并原样转发 work.failed
    And reporter 收到 work.failed

  Scenario S2: 证据不足 gate 打回
    Given U2 只有 2 次尝试记录
    When executor emit work.failed
    Then gate emit work.failed.rejected
    And failed_checks 含 "missing_attempt_record: U2"
    And executor 收到 task.resume

  Scenario S3: 打回后补证据再放行
    Given executor 收到 S2 打回
    When executor 按 failed_checks 补齐 U2 的 4 次尝试后重新 emit work.failed
    Then gate 放行 work.failed

  Scenario S4: 连续 3 次打回耗尽预算
    Given gate 连续 3 次 rejected 同一 work.failed
    When 预算耗尽
    Then runtime 直发 plan.blocked{kind: precheck_exhausted}
    And reporter 走 precheck_exhausted 分支写 blocked 报告并 emit report.done + LOOP_COMPLETE

  Scenario S5: work.failed 带非空 completed_units 机械 reject
    When executor emit work.failed 且 completed_units=["U1"]
    Then policy-check 以 payload_consistency:work-failed-with-completed-units reject
    And 不进入 gate 轮次

  Scenario S6: fix_status:blocked 带非空 completed_fix_units 机械 reject
    When fixer emit fix.done{fix_status: blocked, completed_fix_units: ["U1"]}
    Then policy-check 以 payload_consistency:fix-done-blocked-with-completed-fix-units reject

  Scenario S7: fix.done applied gate 直接放行
    When fixer emit fix.done{fix_status: applied, settlement_confidence: 95, ...}
    Then gate 不做证据审核直接转发 fix.done

  Scenario S8: 置信度虚高被打回
    Given 证据文件仅 2 次单角度尝试但 confidence=92
    When executor emit work.failed
    Then gate rejected 且 failed_checks 含 "confidence_inflated"

  Scenario S9: 缺 dead_end_confidence 字段 schema reject
    When executor emit work.failed 未携带 dead_end_confidence
    Then policy-check 以 schema required_fields 缺失 reject

Feature: F2 rerun 复用增强

  Scenario S10: plan-reviewer report-first 经验挖掘
    Given worktree 存在 .ralph/agent/resume-context.md 且归档含上轮 report.md
    When plan-reviewer 执行 Step 2.25
    Then reuse-guidance.md 含 "Last run terminal state" 节（上轮 verdict/completed+failed units/residuals）
    And 经验骨架来自归档 report.md 与 report-input.*.json 而非全量重挖

  Scenario S11: fixer 经 resume-context 读归档 decisions
    Given resume-context.md 指向的归档 decisions.md 含 "fixer checkpoint: U2 committed=..." 与 failure briefs
    When fixer 执行 re-entry reconciliation
    Then 已提交 fix Unit 计入 completed_fix_units 不重复实现
    And failure briefs 用于避免机械重复

  Scenario S12: 无交集脏文件降级继续
    Given worktree 有不可归因脏文件且与全部剩余 Unit 的 allowed files 无交集
    When executor 执行 Step 1.25
    Then 记录 foreign_dirt（不删/不动/不提交）并继续执行
    And baseline-verification.md 声明 foreign_dirt 路径清单
    And 不 emit work.failed{unattributable_dirty_worktree}

  Scenario S13: 有交集冲突脏文件维持 fail
    Given 脏文件与某剩余 Unit 的 allowed files 有交集且语义冲突
    When executor 执行 Step 1.25
    Then 维持 work.failed{unattributable_dirty_worktree}（现状行为）
```

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐层级 | 风险补充 |
|---|---|---|---|---|
| S1/S3 | proposed→gate pass→reporter 全链 | 新 BDD `ce_executor_pipeline_fail_gate_rejected_then_pass.yml` | BDD（run_workflow_guard_scenario） | gate mock 需读 evidence 文件的提示写进 checklist |
| S2/S4 | rejected 打回 + 耗尽 plan.blocked | 新 BDD `ce_executor_pipeline_fail_gate_exhaust.yml`（模板对齐 `2026-07-02-precheck-gate-exhaust.yml`） | BDD | expected.events 断言 plan.blocked；absent 断言 work.failed |
| S5/S6 | 2 条新 consistency 规则 reject | `event_policy.rs` 邻近表驱动测试（对齐 7718 起既有 payload_consistency 测试模式） + `ralph preset check` | 单元+lint | Mutation：删掉规则则测试必败 |
| S7 | applied 放行不经证据审核 | 新 BDD（rejected_then_pass 场景内 fix.done applied 段） | BDD | — |
| S8/S9 | gate checklist 含 inflated 检查；schema 拒缺失字段 | checklist 文本入 precheck rules（结构化 parse 断言：`RalphConfig::parse_yaml` 后 rules 长度/on_fail 字段）；schema 缺失由 S9 单测 | 单元 | 遵守 Preset 测试规则：不锁文本，只测结构 |
| 模板 | rubric/证据模板注册与落盘 | `cargo nextest run -p ralph-cli --bin ralph -- builtin_artifact_templates` + materialize 冒烟 | 单元+冒烟 | NAMES/数组/build.rs/磁盘四处同名同序 |
| S10-S13 | instructions 行为变更 | 无自动化（prompt 层）；由 preset_lint strict + AAF review 覆盖 | lint+review | 不新增文本包含断言测试 |
| 全量 | preset strict lint + parity | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` + `cargo nextest run -p ralph-core -- preset_lint` + `cargo nextest run -p ralph-cli --bin ralph -- presets` | lint | EGRESS_MAX_HOPS 超跳在此暴露 |
| 回归 | 全量 | `./scripts/run-tests.sh` | 全量 | — |

---

## 6. 需求—测试追踪矩阵

| Req ID | 需求 | Scenario | 测试入口 |
|---|---|---|---|
| R1 | fail 阈值门禁（字段+gate 审核） | S1/S2/S3/S8/S9 | BDD×2 + parse_yaml 结构断言 |
| R2 | 机械形状门禁 | S5/S6 | event_policy 表驱动单测 |
| R3 | gate 打回循环（failed_checks + task.resume） | S2/S3/S4 | BDD×2 |
| R4 | 自评/他评同源 rubric（模板单点维护） | S1/S8 | builtin_artifact_templates 单测 + checklist 结构断言 + AAF review |
| R5 | precheck_exhausted reporter 分支 | S4 | BDD（reporter mock 消费 plan.blocked） |
| R6 | plan-reviewer report-first | S10 | AAF review（prompt 层） |
| R7 | fixer 归档经验链 | S11 | AAF review（prompt 层） |
| R8 | executor foreign_dirt 降级 | S12/S13 | AAF review（prompt 层） |

---

## 7. 严格串行开发单元

```text
U1（event_policy：schema 新字段 + 2 条机械门禁规则）
  ↓
U2（rubric/证据模板创建 + 注册接线：templates 目录 + builtin_artifact_templates.rs + build.rs）
  ↓
U3（event_loop.precheck 块 + 派生拓扑核对：EGRESS_MAX_HOPS / exempt_topics / reporter exhaust 分支前置验证）
  ↓
U4（executor instructions：materialize 引用 rubric + dead-end-evidence 格式 + Rejected-Resume + foreign_dirt）
  ↓
U5（fixer instructions：settlement 引用 rubric + Rejected-Resume + re-entry 归档经验链）
  ↓
U6（plan-reviewer Step 2.25 report-first + reporter precheck_exhausted 分支）
  ↓
U7（BDD×2 + CLAUDE.md/AGENTS.md/index.json 同步 + 全量验证）
```

### U1：event_policy —— schema 新字段 + 机械门禁规则

1. **Unit 目标：** `work.failed` / `fix.done` schema 增加置信度与证据 required 字段（含 field_docs）；`payload_consistency` 增加 2 条形状规则。
2. **对应：** R1/R2；S5/S6/S9；KTD2/KTD3；E1/E11。
3. **外部可观察结果：** 缺新字段的 `work.failed`/`fix.done` 被 policy-check schema reject；completed 非空仍 fail 被 consistency reject。
4. **当前行为基线：** yml:679-741（work.failed 12 个 required）、yml:1286-1320（fix.done 28 个 required）、yml:121-146（3 条规则）。
5. **输入与输出：** YAML schema 段；reject message 指明修复方向。
6. **修改位置：** `presets/en/ce-executor-pipeline.yml`（`event_policy.schemas.work.failed` / `.fix.done` / `payload_consistency.rules`）。
7. **可依赖能力：** 既有 field_docs / rules 结构（同文件既有模式）。
8. **禁止依赖的未来能力：** 不写 precheck 块（U3）；不改 instructions（U4-U6）。
9. **验收：**
   - `work.failed` required_fields += `dead_end_confidence`（int 0-100）、`dead_end_evidence_coverage`（int 0-100）、`dead_end_evidence_file`（repo-relative path）；每字段 field_docs 三件套（meaning/source/fill_rule），fill_rule 禁止伪造路径与虚报数字。
   - `fix.done` required_fields += `settlement_confidence` / `settlement_evidence_coverage` / `settlement_evidence_file`；field_docs 注明：`applied` 时 confidence 语义为「修复完成结论的置信度」、`settlement_evidence_file` 可填 `.ralph/agent/decisions.md`；`partial`/`blocked` 时必须指向按格式写的 `.ralph/review/<plan>/fix-settlement-evidence.md`。
   - 新规则 1 `work-failed-with-completed-units`：`{topic: work.failed, when: {all: [{field: completed_units, non_empty: true}]}}` → message 指引改 `work.done` partial。
   - 新规则 2 `fix-done-blocked-with-completed-fix-units`：`{topic: fix.done, when: {all: [{field: fix_status, eq: blocked}, {field: completed_fix_units, non_empty: true}]}}` → message 指引 `fix_status: partial`。
   - 规则注释写明 guards（只对「真违禁形状」开火）与既有 3 条注释风格一致。
10. **Acceptance Red：** `ralph emit work.failed`（缺新字段）被 schema reject；两条新规则的手工 policy-check 复现 reject。自动化：event_policy 表驱动单测先写（引用规则 id 断言 reject）——初跑 Miss（规则不存在）。
11. **单元测试拆分：** 规则 1 命中/未命中（completed 空）×2；规则 2 命中/未命中（fix_status=applied）×2；schema 缺字段 reject ×2。
12. **Red → Green → Refactor：** 单测 Red → yml 规则+字段 Green → `ralph preset check` 结构化校验 Green。
13. **最小实现范围：** 仅上述 YAML 段；不动其他任何文件。
14. **集成验证：** `cargo nextest run -p ralph-core -- event_policy` + `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`。
15. **风险驱动测试：** Mutation（删规则必败）；类型不匹配 fail-close（confidence 填字符串 → Hit）。
16. **回归范围：** `cargo nextest run -p ralph-core -- event_policy`（既有 3 条规则测试全绿）。
17. **预期文件变更：** `presets/en/ce-executor-pipeline.yml`。Evidence：E1/E11。
18. **完成标准：** 新单测全绿 + preset_lint 绿 + 可独立提交。
19. **停止条件：** lint 对新规则本身报 finding（如 when 字段引用未声明字段——停并核对 `collect_referenced_fields` 的 lint 覆盖）。
20. **风险与注意：** fix.done 的 28 个既有 required 字段不动；新增字段名不得与既有冲突（grep 确认 `settlement_confidence` 等无既有占用）。

### U2：rubric/证据模板创建 + 注册接线

1. **Unit 目标：** 创建 `presets/templates/ce-executor-pipeline/` 三个模板文件并完成注册三件套（NAMES 常量 + include_str 数组 + 匹配分支 + build.rs 拷贝），`ralph preset materialize-artifacts ce-executor-pipeline --plan-key <key>` 可落盘。
2. **对应：** R4；S1/S2/S8 的 rubric 载体；KTD11；E13/E14。
3. **外部可观察结果：** materialize 命令对新 preset 成功落盘 3 个模板文件；未知 preset 报错路径不受影响。
4. **当前行为基线：** `templates_for_preset` 仅匹配 `parallel-forge` / `red-team-attack`（builtin_artifact_templates.rs:183-189），其他 preset 报 unknown-preset 错（:340 测试锚点）。
5. **输入与输出：** 模板源文件；`materialize()` 返回落盘路径列表。
6. **修改位置：** `presets/templates/ce-executor-pipeline/`（3 个新文件）、`crates/ralph-cli/src/builtin_artifact_templates.rs`、`crates/ralph-cli/build.rs`。
7. **可依赖能力：** U1 的字段名（rubric 引用 `dead_end_confidence` / `settlement_confidence` 等）；parallel-forge 注册模式（E13）。
8. **禁止依赖的未来能力：** 不写 precheck 块（U3）；不改任何 hat instructions（U4-U6）。
9. **验收：**
   - `fail-confidence-rubric.template.md` 内容四节：① 四维度评分对照表（尝试充分性：每 Unit 1 初始+3 retry 共 4 次，每少 1 次 −15；假设多样性：≥3 角度，每重复角度 −10；因果链完整性：缺 trigger→中间步骤→症状 全链 + file:line 则上限 70；假因排除：未排除环境/依赖/flake/baseline 则上限 80）；② 阈值表（≥90 允许 fail / 75-89 打回补强 / 60-74 继续 retry / <60 禁止 fail）；③ coverage 计算规则（有可复核来源——file:line / 命令输出摘录 / 日志片段 / 测试名 / 文档路径——的 claim ÷ 总 claim ×100；≥75 合格，<75 须写缺口解释）；④ 六类 failed_checks 定义与各自含义（`missing_attempt_record` / `single_angle_retries` / `broken_causal_chain` / `unverifiable_evidence` / `confidence_inflated` / `uneliminated_alternatives`）。模板内注明适用字段名（executor 用 `dead_end_*`、fixer 用 `settlement_*`）。
   - `settlement-evidence.template.md`：证据文件格式——每个 failed/blocked Unit 一节（4 次尝试记录：角度+失败摘要；最后假设因果链 + file:line；假因排除记录；证据来源清单）+ 文末自评两个数字及打分理由；注明 executor 落盘 `.ralph/review/<plan>/dead-end-evidence.md`、fixer 落盘 `.ralph/review/<plan>/fix-settlement-evidence.md`。
   - `README.md`：目录说明（对齐 parallel-forge/README.md 惯例）。
   - 注册三件套：`CE_EXECUTOR_PIPELINE_TEMPLATE_NAMES`（3 个 basename）、`CE_EXECUTOR_PIPELINE_TEMPLATES` include_str 数组、`templates_for_preset` 加 `"ce-executor-pipeline" => Ok(CE_EXECUTOR_PIPELINE_TEMPLATES)` 分支、build.rs `copy_preset_templates(&templates_root, &out_dir, "ce-executor-pipeline", ...)` 调用（签名对齐既有调用点）。
   - 测试（对齐既有模式）：NAMES 与数组长度一致、materialize 落盘 3 文件、幂等、unknown preset 报错仍覆盖非注册名。
10. **Acceptance Red：** 新测试引用 `CE_EXECUTOR_PIPELINE_TEMPLATE_NAMES` / `materialize("ce-executor-pipeline", ...)` —— 编译失败或 unknown-preset Err（常量/分支不存在）。
11. **单元测试拆分：** NAMES 同步 ×1、落盘文件数 ×1、幂等 ×1、报错路径 ×1。
12. **Red → Green → Refactor：** 测试 Red → 模板文件 + 注册三件套 Green → build.rs 拷贝验证（`cargo build -p ralph-cli` 后 materialize 落盘内容逐字等于源文件）。
13. **最小实现范围：** 仅 3 个模板文件 + 两个 rust 文件的注册段；不改其他任何 preset / 不动 `templates_for_preset` 既有分支。
14. **集成验证：** `cargo nextest run -p ralph-cli --bin ralph -- builtin_artifact_templates` + `cargo build -p ralph-cli`。
15. **风险驱动测试：** 模板内容含占位符（`<plan>` 等）不被 materialize 误替换（materialize 是纯拷贝，对齐 E13 语义）。
16. **回归范围：** `cargo nextest run -p ralph-cli --bin ralph -- builtin_artifact_templates`（parallel-forge / red-team-attack 既有测试全绿）。
17. **预期文件变更：** 3 新模板 + `builtin_artifact_templates.rs` + `build.rs`。Evidence：E13/E14。
18. **完成标准：** 新测试全绿 + 既有模板测试全绿 + `ralph preset materialize-artifacts ce-executor-pipeline --plan-key smoke` 手工冒烟落盘成功（冒烟后删除临时目录）+ 可独立提交。
19. **停止条件：** build.rs 的 `copy_preset_templates` 签名与既有调用点不同构（停并核对该函数参数表）；include_str 路径拼接规则与 OUT_DIR 结构不符（编译错即停，核对 build.rs 拷贝目标布局）。
20. **风险与注意：** NAMES 常量、include_str 数组、build.rs 拷贝、磁盘文件四处必须同名同序（E13 注释明确 keep-in-sync）；模板内容是给 agent 看的操作文档，遵循「触发条件/动作/失败停止条件」写法，不写 runtime 内部实现。

### U3：event_loop.precheck 块 + 派生拓扑核对

1. **Unit 目标：** preset 新增 `event_loop.precheck` 块（2 条 rules）；完成派生拓扑三项核对（EGRESS_MAX_HOPS、exempt_topics 交互、gate 转发校验路径），必要时调常量。
2. **对应：** R1/R3/R5；S1-S4/S7；KTD1/KTD4/KTD7/KTD8；E2/E3/E4/E6/E9/E10。
3. **外部可观察结果：** `RalphConfig::parse_yaml` 后 precheck rules 就位；normalize 后 hats 含 `precheck-work.failed` / `precheck-fix.done` 两个合成 gate。
4. **当前行为基线：** preset 无 precheck 块；EGRESS_MAX_HOPS=12 恰覆盖现链（E9）。
5. **输入与输出：** YAML `event_loop.precheck`；gate hat 的 checklist instructions（`build_gate_instructions` 渲染 `rules.<X>.prompt`）。
6. **修改位置：** `presets/en/ce-executor-pipeline.yml`（event_loop 段 + 头部拓扑注释 14→16 hat）；条件性 `workflow_activation.rs:465`。
7. **可依赖能力：** U1 的字段名（checklist 引用）；U2 的模板（checklist ① 的 materialize 步骤）；脱糖/enforcement 既有实现。
8. **禁止依赖的未来能力：** 不写 producer 侧 instructions（U4-U6）；不写 BDD（U7）。
9. **验收：**
   - YAML 块：
     ```yaml
     event_loop:
       precheck:
         enabled: true
         rules:
           work.failed:
             prompt: <6 条 checklist（见下）>
             on_fail: {target: executor, retry_budget: 3, on_exhausted: "plan.blocked(reason=precheck_failed)", reason: "dead_end_evidence_insufficient"}
           fix.done:
             prompt: <4 条 checklist（applied 直通 + partial/blocked 同标准审核）>
             on_fail: {target: fixer, retry_budget: 3, on_exhausted: "plan.blocked(reason=precheck_failed)", reason: "fix_evidence_insufficient"}
     ```
   - work.failed checklist（6 条）：① 先 `ralph preset materialize-artifacts ce-executor-pipeline --plan-key <plan_name>`，从落盘 templates 目录读 `fail-confidence-rubric.template.md` 与 `settlement-evidence.template.md`，本次审核以该模板为唯一标准；② 打开 payload 的 `dead_end_evidence_file`，不存在/空/未按模板格式 → rejected；③ 对照 `failed_units`+`blocked_units` 逐 Unit 查 rubric §1 四维度证据（尝试次数/角度/因果链/假因排除），缺口按 rubric §4 归类为对应 failed_check；④ 抽查 ≥2 条证据来源可复核（打开文件/重跑命令），无来源断言 → `unverifiable_evidence`；⑤ `dead_end_confidence ≥90` 且与 rubric §1 证据质量匹配（虚高 → `confidence_inflated`），`dead_end_evidence_coverage` 达标按 rubric §3 判定；⑥ 全过 → 原样转发 emit `work.failed`（payload 逐字段复制，不得改写），任一不过 → `work.failed.rejected`，failed_checks 用 rubric §4 的六类命名并附具体缺口。
   - fix.done checklist（4 条）：① 同 work.failed ①（materialize 读模板）；② `fix_status=applied` → 直接原样转发；③ `partial`/`blocked` → 打开 `settlement_evidence_file` 按 rubric 同标准审核 `failed_fix_units`+`blocked_fix_units`（字段名换 `settlement_*`）；④ 全过原样转发，不过 emit `fix.done.rejected`（failed_checks 同六类命名）。
   - 核对点 A（EGRESS_MAX_HOPS）：跑 `cargo nextest run -p ralph-core -- preset_lint` + `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`；若 WAC 报 ce-executor-pipeline 超跳，`workflow_activation.rs:465` 常量 12→13 并在头部注释更新跳数说明（放宽只增通过面，不回破其他 preset——用全量 preset_lint 证明）。
   - 核对点 B（exempt_topics）：`ralph -c presets/en/ce-executor-pipeline.yml inspect prompt --hat executor --format json` 确认脱糖后 executor 的 emit allowlist/豁免对 `.proposed` 变体生效；若失效（emit 被 ACL 拦），停并升级问题（可能需要 exempt_topics 同步改写——属 runtime 行为缺口，超本计划范围时降级为 known-issue 文档化）。
   - 核对点 C（gate 转发校验）：写一个最小 BDD（producer→proposed→gate 转发→consumer）验证 gate emit 真 `work.failed` 过 schema（payload 原样转发字段齐全）。
10. **Acceptance Red：** `parse_yaml` 后断言 precheck rules=2、normalize 后 hats 含两个 `precheck-*` gate 且 `max_activations=4` —— 初跑无块（测试红）。
11. **单元测试拆分：** 结构化 parse 断言（rules 数、on_fail 四字段、prompt 非空）——遵守 Preset 测试规则（测结构不锁文本）。
12. **Red → Green → Refactor：** 结构断言 Red → YAML 块 Green → lint 核对 A → 常量调整（如需要）→ 全量 lint Green。
13. **最小实现范围：** 仅 event_loop 段 + 头注释 + 条件性常量；不动 instructions。
14. **集成验证：** `cargo nextest run -p ralph-cli --bin ralph -- presets`（parity + strict lint）+ `cargo nextest run -p ralph-core -- ralph_config`（desugar 测试不受影响）。
15. **风险驱动测试：** `RALPH_PRECHECK_MODE=off` 时 parse/normalize 回退现状（既有 kill-switch 测试应仍绿）。
16. **回归范围：** `cargo nextest run -p ralph-core -- config` + `cargo nextest run -p ralph-core -- precheck`。
17. **预期文件变更：** `presets/en/ce-executor-pipeline.yml`；条件性 `workflow_activation.rs`。Evidence：E2/E3/E9。
18. **完成标准：** 结构断言全绿 + strict lint 全绿（含跳数）+ 最小 BDD 验证 gate 转发过 schema。
19. **停止条件：** 核对点 B 发现 exempt_topics 对 `.proposed` 失效且需要 runtime 改动——停，与 operator 确认是否接受 known-issue 降级（emit 时改用 `ralph emit work.failed.proposed` 显式写法）或扩大范围。
20. **风险与注意：** 头部注释的 hat 数、链描述、EGRESS_MAX_HOPS 说明三处同步；`topic_deny_rules` 无需为派生 topic 补行（E5）但要在注释里说明依据。

### U4：executor instructions —— materialize 引用 rubric + dead-end-evidence + Rejected-Resume + foreign_dirt

1. **Unit 目标：** executor instructions 四处升级：Confidence Protocol 改为「materialize → 读 rubric 模板 → 按模板自评」、dead-end-evidence.md 按模板写、Rejected-Resume 纪律、Step 1.25 foreign_dirt 降级。
2. **对应：** R1/R3/R4/R8；S1/S2/S3/S8/S12/S13；KTD3/KTD10/KTD11；E10/E12/E14。
3. **外部可观察结果：** 无（prompt 层行为）。
4. **当前行为基线：** Confidence Protocol yml:2700-2704（无阈值无强制）；Failure Path yml:2674-2698；Step 1.25 yml:2248-2299。
5. **输入与输出：** instructions 文本；agent 行为变化。
6. **修改位置：** `presets/en/ce-executor-pipeline.yml` executor `instructions:` 段。
7. **可依赖能力：** U1 字段名、U2 模板（落盘后可读）、U3 gate 行为（打回形状）。
8. **禁止依赖的未来能力：** 不改 fixer/plan-reviewer/reporter。
9. **验收（文本要点）：**
   - **Confidence Protocol 升级**（替换 yml:2700-2704）：emit `work.failed` 前先 `ralph preset materialize-artifacts ce-executor-pipeline --plan-key <plan_name>`，从落盘 templates 目录读 `fail-confidence-rubric.template.md`；按 rubric §1 四维度自评 `dead_end_confidence`、按 §3 自评 `dead_end_evidence_coverage`；阈值判定按 §2（≥90 才允许 emit `work.failed`；75-89 先验证关键假设；60-74 继续 retry 换角度；<60 禁止 fail）。instructions 不复述 rubric 内容（模板单点维护，KTD11）。
   - **dead-end-evidence.md 格式**：emit 前必写，落 `.ralph/review/<plan>/dead-end-evidence.md`，格式按落盘的 `settlement-evidence.template.md` 复制填写（不复述格式全文）。
   - **Rejected-Resume 纪律**（新增小节）：emit 后会经一轮证据审核（precheck gate，见 `ralph-tools-precheck`）；被打回（task.resume 带 failed_checks）后按 rubric §4 六类缺口行动——`missing_attempt_record` → 回 Execution Loop 用残留预算继续 retry；`single_angle_retries` → 读 decisions.md failure briefs + reuse-guidance 的 `Do not mechanically repeat`，从 `Fresh options for this run` 选新角度；`broken_causal_chain` → 先补诊断再动手；`unverifiable_evidence` → 补可复核来源；`confidence_inflated` → 重新自评，不够就继续干活；`uneliminated_alternatives` → 重跑 baseline/隔离环境复验。3 次打回耗尽后 runtime 发 `plan.blocked{kind: precheck_exhausted}`，不得自行重试或绕过审核。引用 `ralph-tools-precheck` skill（producer 行为段），不复述。
   - **Step 1.25 新增 foreign_dirt 分支**：不可归因脏文件先做交集检查——与全部剩余 Unit 的 allowed files 无交集 → 记录 `executor re-entry foreign_dirt: <paths>` 到 decisions.md + 在 baseline-verification.md 声明清单，不删/不动/不提交，继续执行；有交集且语义冲突 → 维持 `work.failed{unattributable_dirty_worktree}` 现状。
   - emit 示例 payload 补三个新字段（完整示例与 partial 示例同步）。
10. **Acceptance Red：** 无自动化 Red（prompt 层）；以 AAF 五问表自检 + preset_lint 绿为门。
11. **单元测试拆分：** 无（遵守 Preset 测试规则，不锁文本）。
12. **Red → Green → Refactor：** 草稿 → 对照 U1 字段名 / U2 模板节号 / U3 gate 行为逐条核对 → `ralph preset check`。
13. **最小实现范围：** executor instructions 段 only。
14. **集成验证：** `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`。
15. **风险驱动测试：** 无。
16. **回归范围：** preset_lint + parity。
17. **预期文件变更：** `presets/en/ce-executor-pipeline.yml`。Evidence：E10/E12/E14。
18. **完成标准：** instructions 引用的字段名、模板文件名、materialize 命令、六类 failed_checks 与 U1/U2/U3 逐字对齐 + lint 绿 + 可独立提交。
19. **停止条件：** 发现 gate 行为与 U3 定义不一致（停并回 U3 修订）；模板节号与 U2 实际文件不符（停并回 U2 修订）。
20. **风险与注意：** instructions 视角规则（HARD RULE 4）——只说 executor 自己可见可调用的；gate 用「emit 后会经一轮证据审核」的 producer 可见语言表述并引用 ralph-tools-precheck，不出现脱糖/合成等 runtime 内部词；rubric 与证据格式只给 materialize 路径，不复述（KTD11）。

### U5：fixer instructions —— settlement 引用 rubric + Rejected-Resume + re-entry 归档经验链

1. **Unit 目标：** fixer instructions 三处升级：Settlement Confidence 段（新增，materialize 引用 rubric 模板）、Rejected-Resume 纪律、Step 1 re-entry 经 resume-context 读归档 decisions.md。
2. **对应：** R1/R3/R4/R7；S6/S7/S11；KTD3/KTD4/KTD11；E7/E10/E12/E14。
3. **外部可观察结果：** 无（prompt 层）。
4. **当前行为基线：** fixer 无 Confidence 段；re-entry yml:4589-4598（只读当前 decisions.md）；无 gate 相关指引。
5. **输入与输出：** instructions 文本。
6. **修改位置：** `presets/en/ce-executor-pipeline.yml` fixer `instructions:` 段。
7. **可依赖能力：** U1 字段名、U2 模板（落盘后可读）、U3 gate 行为、U4 文本结构（同构复用）。
8. **禁止依赖的未来能力：** 不改其他 hat。
9. **验收（文本要点）：**
   - **Settlement Confidence 段**（新增）：`partial`/`blocked` 结算前先 `ralph preset materialize-artifacts ce-executor-pipeline --plan-key <plan_name>` 读 `fail-confidence-rubric.template.md`，按 §1/§3 自评 `settlement_confidence` 与 `settlement_evidence_coverage`，阈值判定按 §2（≥90 才允许 `partial`/`blocked`）；证据文件按 `settlement-evidence.template.md` 复制填写，落 `.ralph/review/<plan>/fix-settlement-evidence.md`；`applied` 时 `settlement_evidence_file` 可填 `.ralph/agent/decisions.md`。不复述 rubric/格式全文（KTD11）。
   - **Rejected-Resume 纪律**：同 U4 六类缺口表（fix Unit 语境）；引用 `ralph-tools-precheck`。
   - **Step 1 re-entry 增强**：当前 decisions.md 无 `fixer checkpoint:` 时，检查 `.ralph/agent/resume-context.md`——存在则读其指向的归档 decisions.md，提取 `fixer checkpoint:` 行与 failure briefs；已提交 fix Unit 计入 `completed_fix_units` 不重复实现；failure briefs 仅作避免机械重复的参考，不消耗本轮 retry 预算。
   - emit 示例 payload 补三个新字段。
10. **Acceptance Red：** 同 U4（prompt 层无自动化）。
11-20. 同 U4 结构（lint 门、同源对齐、视角规则、可独立提交）。

### U6：plan-reviewer Step 2.25 report-first + reporter precheck_exhausted 分支

1. **Unit 目标：** plan-reviewer 复用经验挖掘改 report-first；reporter 新增 `kind=precheck_exhausted` 的 plan.blocked 消费分支。
2. **对应：** R5/R6；S4/S10；KTD7/KTD9；E6/E7/E12。
3. **外部可观察结果：** 无（prompt 层）。
4. **当前行为基线：** Step 2.25 yml:1710-1747（无 report 优先级）；reporter plan.blocked 分支假设 report_input_file 存在（E6）。
5. **输入与输出：** instructions 文本。
6. **修改位置：** `presets/en/ce-executor-pipeline.yml` plan-reviewer / reporter `instructions:` 段。
7. **可依赖能力：** U3 的 on_exhausted 形状（`{topic, reason, kind: precheck_exhausted}`）。
8. **禁止依赖的未来能力：** 不改归档机制（runtime）。
9. **验收（文本要点）：**
   - **Step 2.25 report-first**：归档目录存在时，第一步定位最新 `reuse-history/*/review/<plan_name>/report.md` 与同目录 `report-input.*.json`，以其为经验骨架；再用归档 decisions.md / handoff.md / summary.md 补充细节。reuse-guidance.md 新增首节 `Last run terminal state`（上轮 verdict / completed+failed units / 主要 residuals / report 路径），随后四节照旧。找不到 report（上轮未跑到 reporter）时降级为现状挖掘流程并在 guidance 注明。
   - **reporter precheck_exhausted 分支**：trigger 为 `plan.blocked` 且 payload 含 `kind: precheck_exhausted`（无 plan_name/report_input_file）时，不尝试读 report bundle；写简版报告（`.ralph/review/precheck-exhausted-<ts>/report.md`）记录：被 guard 的 topic（payload.topic）、连续打回次数、reason；`report.done{verdict: blocked}` + `LOOP_COMPLETE`。
10. **Acceptance Red：** 同 U4。
11-20. 同 U4 结构。

### U7：BDD×2 + 文档同步 + 全量验证

1. **Unit 目标：** 新增 2 个 BDD 场景；同步 CLAUDE.md/AGENTS.md/index.json；全量验证收尾。
2. **对应：** R1-R8 全部；S1-S9；E8。
3. **外部可观察结果：** 新场景在 `cargo nextest run -p ralph-core --test scenarios` 下通过。
4. **当前行为基线：** precheck 场景模板 `2026-07-02-precheck-gate-{pass,exhaust}.yml`（E8）；旧 ce_executor_pipeline_* 场景独立 config 不受影响。
5. **输入与输出：** 场景 yml + scenarios.rs 注册；文档三处。
6. **修改位置：** `crates/ralph-core/tests/scenarios/`（新增 2 文件）、`crates/ralph-core/tests/scenarios.rs`、`CLAUDE.md`、`AGENTS.md`、`presets/index.json`。
7. **可依赖能力：** U1-U6 全部落地的 preset。
8. **禁止依赖的未来能力：** 无。
9. **验收：**
   - `ce_executor_pipeline_fail_gate_rejected_then_pass.yml`：精简拓扑（executor→precheck-work.failed gate→reporter），mock 序列——executor emit `work.failed.proposed`（带新三字段）→ gate emit `work.failed.rejected{failed_checks, reason}` → （task.resume 后）executor 再 emit `work.failed.proposed` → gate 转发 `work.failed` → reporter emit `report.done` + `LOOP_COMPLETE`；expected.events 含 rejected 与最终 work.failed。
   - `ce_executor_pipeline_fail_gate_exhaust.yml`：mock 连续 3 次 rejected → expected.events 含 `plan.blocked`（kind=precheck_exhausted）；absent_events 含 `work.failed`；completion true。
   - 场景 config 内联 precheck 块与 schema 新 required 字段（对齐模板风格）。
   - CLAUDE.md/AGENTS.md 的 ce-executor-pipeline 描述行：14-hat → 16-hat（含 2 合成 precheck gate）+ fail 门禁与 rerun 增强一句话；两文件 `cp` 同步（HARD RULE）。
   - index.json 描述行补「evidence-gated fail + rerun reuse guidance」（行为描述同步，无结构化字段）。
   - 全量：`./scripts/run-tests.sh`。
10. **Acceptance Red：** 新场景初跑失败（gate mock 的 proposed payload 缺新 required 字段 → schema reject；或 exhaust 场景 plan.blocked 未出现）。
11. **单元测试拆分：** 两个场景各一函数。
12. **Red → Green → Refactor：** 场景 Red → 对照 U1/U3 实际字段/规则修正 mock → Green → 文档同步 → 全量。
13. **最小实现范围：** 上述 5 个文件。
14. **集成验证：** `cargo nextest run -p ralph-core --test scenarios` + `./scripts/run-tests.sh`。
15. **风险驱动测试：** exhaust 场景 absent_events 断言（work.failed 绝不落盘）。
16. **回归范围：** 全量（含旧 ce_executor_pipeline_* 场景全绿）。
17. **预期文件变更：** 2 新场景 + scenarios.rs + CLAUDE.md + AGENTS.md + index.json。Evidence：E8。
18. **完成标准：** 全量绿 + 文档同步完成 + 可独立提交。
19. **停止条件：** 旧场景意外失败（说明 preset 改动有未预期外溢——停并定位）。
20. **风险与注意：** mock 的 `work.failed.proposed` 直接写派生 topic（对齐既有模板）；场景必须走 `run_workflow_guard_scenario`（断言 events），禁止 `run_scenario` stub。

---

## 8. Unit 串行依赖图

```text
U1（schema 字段 + 机械规则）
  ↓ 字段名 / 规则 id 被引用
U2（rubric/证据模板 + 注册接线）
  ↓ 模板文件名 / materialize 命令 / rubric 节号
U3（precheck 块 + 拓扑核对）
  ↓ gate 行为 / 打回形状 / 常量结论
U4（executor instructions）──┐
U5（fixer instructions）─────┤ 三者均只依赖 U1/U2/U3，互相独立但同文件，串行提交避免冲突
U6（plan-reviewer + reporter）┘
  ↓
U7（BDD + 文档 + 全量验证）
```

## 9. 执行命令清单

```bash
# U1 验收
cargo nextest run -p ralph-core -- event_policy
# U2 验收（模板注册）
cargo nextest run -p ralph-cli --bin ralph -- builtin_artifact_templates
cargo build -p ralph-cli
ralph preset materialize-artifacts ce-executor-pipeline --plan-key smoke   # 冒烟后删除临时目录
# U3 验收（含 EGRESS_MAX_HOPS 核对）
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- presets
ralph -c presets/en/ce-executor-pipeline.yml inspect prompt --hat executor --format json
# U4-U6 验收
ralph preset check presets/en/ce-executor-pipeline.yml 2>/dev/null || cargo nextest run -p ralph-cli --bin ralph -- preset_lint
# U7 验收
cargo nextest run -p ralph-core --test scenarios
./scripts/run-tests.sh
```

## 10. 最终质量门禁

- [ ] `cargo nextest run -p ralph-core -- event_policy` 全绿（含 2 条新规则表驱动）
- [ ] `cargo nextest run -p ralph-cli --bin ralph -- builtin_artifact_templates` 全绿（新模板注册 + 既有 preset 回归）且 materialize 冒烟落盘成功
- [ ] `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` + `cargo nextest run -p ralph-core -- preset_lint` 全绿（含 EGRESS_MAX_HOPS 核对结论记录）
- [ ] `cargo nextest run -p ralph-cli --bin ralph -- presets`（parity + strict lint）全绿
- [ ] 2 个新 BDD 场景通过且走 `run_workflow_guard_scenario`
- [ ] 旧 `ce_executor_pipeline_*` 场景全绿（无外溢）
- [ ] `./scripts/run-tests.sh` 全量绿
- [ ] CLAUDE.md / AGENTS.md 内容一致且描述行已更新
- [ ] executor/fixer instructions 与 gate checklist 引用的字段名、模板文件名、materialize 命令、六类 failed_checks 与 schema/precheck 块/模板文件逐字对齐；rubric 全文只存在于 `fail-confidence-rubric.template.md`（单点维护）
- [ ] 无 `.ralph/review/<plan>/{residuals*,scratch,draft}` 残留进 git

## 11. 最终计划自检

- [x] 每个 Unit 有目标 / 验收 / Red / 最小范围 / 停止条件
- [x] 全部改动锚定 Evidence（E1-E14），无凭记忆讨论
- [x] runtime 触点仅模板注册接线（无行为改动）+ 条件性 EGRESS_MAX_HOPS（以 lint 实跑为准）
- [x] rubric 模板单点维护（KTD11），instructions/checklist 只引用不复述
- [x] 遵守 Preset 测试规则（无文本锁定测试；结构化语义 + BDD）
- [x] 遵守 hat instructions 视角规则（gate 描述用 producer 可见语言 + 引用 ralph-tools-precheck）
- [x] 下游同步清单逐项落实（BDD / lint / CLAUDE.md+AGENTS.md / index.json；manifest/schemas/zsh 不涉及并说明理由）
- [x] 非目标明确（runtime 归档摘要、retry 次数、loop preset、coverage runtime 门槛、lt 操作符、模板专用别名命令）
