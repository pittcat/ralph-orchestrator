---
date: 2026-07-02
topic: ce-executor-pipeline-preset
---

# 线性一条龙执行 Preset（ce-executor-pipeline）

## 问题框架（Problem Frame）

现有 `ce-executor-serial` 是一个 10-hat 的重型 preset：Plan → Execute(TDD) → Validate → Fix → 6 维 Review → Ship → Report，带 per-unit 迭代、独立 validator hat、shipper、以及 coordinator 循环式的维度评审。对于「拿一份计划，想让它自动跑完并出报告」的场景，它的状态机太复杂。

用户想要一个**线性「一条龙」preset**：把一份计划先评审并修好，整份交给执行器一把做完（TDD、全绿），执行完**直接进入串行的多维度评审**——每个维度一个 hat、一环扣一环写好各自产物，最后由一个 synthesizer 综合汇总出一份修复计划，交给修复器全部修复；报告前加一道对齐关核对计划与修复计划是否真的落地，最后出报告。诉求是**事件链严格单链路、单消费者、一环扣一环、无分支回环**。

相对 `ce-executor-serial`，本 preset **砍掉**：per-unit 迭代拆分、独立 validator hat、shipper、coordinator 循环（改成扁平直链）；**保留**：TDD 执行 + 全量测试全绿硬门槛（内建在 executor 里）、6 维度评审 + synthesizer 汇总；**新增**：前置计划评审修复关、报告前对齐关。

关键取舍（用户 2026-07-02 修正）：reviewer **不**用单 hat + 并行 subagent，而是**每个维度一个 hat 的串行链**。代价是 hat 数变多，收益是事件链清晰、纯事件驱动、绕开 subagent 后端依赖。约束：4+ hats 强制 `execution_mode: isolated`。

---

## 参与者（Actors，均为 pipeline-agent hat）

- A1. `plan-reviewer`（前置计划关）：读入计划文件，评审计划质量，就地修复/改进计划文档，产出「定稿计划」后交给执行器。**只做两件事：评审计划 + 修复计划**（针对计划文档本身，不是代码）。
- A2. `executor`（执行器）：接收定稿计划，**整份一把执行**，采用 **TDD 模式**（先写/更新测试，再实现）；完成的硬门槛（DoD）是 **全量测试套件必须全绿**，测试不过不得交棒。完成后触发第一个维度评审。不做 per-unit 拆分。
- A3. `dimension-reviewer`（维度评审，**6 个 hat 的串行链**）：`goal-alignment → correctness → testing → maintainability → project-standards → adversarial`。每个维度一个 hat，只审自己那一维、写好该维度的**评审产物文件**、再把事件交给下一维；一环扣一环、单链路。
- A4. `review-synthesizer`（综合汇总）：最后一维（adversarial）完成后触发，读取全部 6 个维度产物，去重/合并/定级，写出一份「修复计划文件」（`fix_plan_file`），交给修复器。
- A5. `fixer`（修复器）：读修复计划，**全部修复**，交给对齐器。若修复计划为空则为直通（确认无需修复）。
- A6. `alignment`（对齐关）：交叉核对 (a) 定稿计划是否完全执行、(b) 修复计划是否完全执行；**未完成项记录为残留（不回环）**，交给报告器。
- A7. `reporter`（报告器）：汇总全流程，产出面向管理者的完成报告，收尾 `LOOP_COMPLETE`；同时兜底消费失败事件（`plan.blocked` / `work.failed`）。
- A8. `progress-steward`（loop 级 fallback）：`loop.stalled` 时兜底唤醒卡住的 hat，emit 一个恢复事件、N 次后升级 `plan.blocked`。

---

## 关键流程（Key Flows）

- F1. 线性一条龙主流程
  - **触发：** `ralph run -H builtin:ce-executor-pipeline -p "<plan.md>"`
  - **参与者：** A1 → A2 → A3(×6) → A4 → A5 → A6 → A7
  - **步骤（单消费者、单链路、无分支回环）：**
    ```
    work.start
      → plan-reviewer          → plan.ready            （评审并修好计划文档）
      → executor               → work.done             （TDD 整份执行 + 全量测试全绿硬门槛）
      → dim: goal-alignment    → 写产物 → 下一维
      → dim: correctness       → 写产物 → 下一维
      → dim: testing           → 写产物 → 下一维
      → dim: maintainability   → 写产物 → 下一维
      → dim: project-standards → 写产物 → 下一维
      → dim: adversarial       → 写产物 → 进 synthesizer
      → review-synthesizer     → review.complete       （汇总 6 维产物 → 出 fix_plan_file）
      → fixer                  → fix.done              （按修复计划全部修复）
      → alignment              → align.done            （核对计划/修复计划执行度，附 residuals）
      → reporter               → report.done → LOOP_COMPLETE
    ```
  - **结果：** 计划被改进并执行、代码被 6 维逐一评审并综合成修复计划、全部修复、执行度被对齐核对、产出报告，报告里能看到「计划被怎么改进 / 执行了什么 / 各维度评审发现 / 修了什么 / 还有哪些残留没做完」。
  - **逃逸路径：** plan 不可用 / executor 无法全绿 → 直达 reporter 出受阻报告；对齐关发现未落地项 → 不回环、记残留继续；loop 卡死 → `progress-steward` 兜底。

> 两个「修复」概念不要混淆：
> - **修计划**（A1）= 改进**计划文档**本身，让它更清晰可执行。
> - **修代码**（A4 出计划 + A5 执行）= 执行后对**代码**的多维评审修复。

---

## 需求（Requirements）

**拓扑与执行模式**
- R1. Preset 包含以下功能 hat：`plan-reviewer`、`executor`、6 个 `dimension-reviewer`（goal-alignment/correctness/testing/maintainability/project-standards/adversarial）、`review-synthesizer`、`fixer`、`alignment`、`reporter`（共 12 个）+ 1 个 loop 级 fallback（`progress-steward`）。
- R2. 事件拓扑严格单链路：每个业务事件恰好一个消费者（`triggers`），无多消费者、无分支、无回环；6 个维度 hat 串行一环扣一环。
- R3. `execution_mode: isolated`（4+ hats 项目硬规则强制），并通过 `preset_lint::check_multi_hat_isolation`。

**计划关（A1）**
- R4. `plan-reviewer` 读入 `-p` 指定的计划文件，先评审计划质量，再就地修复/改进计划文档，产出「定稿计划」交给 `executor`。
- R5. `plan-reviewer` 不拆 unit、不写代码，只处理计划文档本身。

**执行（A2）**
- R6. `executor` 整份一把执行，采用 **TDD 模式**（先写/更新测试再实现），不做 per-unit 迭代拆分。
- R7. `executor` 的完成标准（DoD）= **全量测试套件必须全绿**（build 通过 + 全部测试通过）；测试不过不得进入评审阶段。硬门槛内建在 executor 里（不单独起 validator hat）。

**多维评审 + 汇总（A3、A4）**
- R8. 评审为 **6 个 dimension-reviewer hat 的串行链**（每维一个 hat）。每个维度 hat 触发于上一维的完成事件，只审自己那一维，把该维度发现写成**评审产物文件**，再 emit 事件触发下一维；每个 hat 一轮**只 emit 一个**事件（isolated 单 emit）。评审必须实际运行测试复核正确性（testing/correctness 维度承担）。
- R9. `review-synthesizer` 在最后一维（adversarial）完成后触发，读取全部 6 个维度产物，去重/合并/跨维一致性提升并按 P0-P3 定级，汇总为一份 `fix_plan_file`（含证据与建议修法）交给 `fixer`；即使无问题也产出「无需修复」的空计划；**emit 恰好一个** `review.complete`。
- R10. `fixer` 依据修复计划执行全部修复；修复计划为空时为直通并显式确认「无需修复」。

**对齐关（A6）**
- R11. `alignment` 交叉核对定稿计划与修复计划的实际执行度（对照代码改动 / 进度证据）。
- R12. 对齐发现的未落地项一律记为残留（residuals），**不触发回环、不重试、不阻断**，继续交给 `reporter`。

**报告（A7）**
- R13. `reporter` 汇总计划改动、执行、各维度评审发现、修复、对齐残留，产出面向管理者的完成报告，并收尾 `LOOP_COMPLETE`。

---

## 成功标准（Success Criteria）

- 人类：给一份计划，一条龙自动跑完并产出报告；报告能清晰回答「计划被怎么改进、执行了什么、6 个维度各发现了什么、修复了什么、还有哪些残留没做完」。
- 下游 agent handoff：`ce-plan` / `ralph-hats` 拿到本需求能直接落 YAML，不需要再发明 hat 职责或事件流；每个 hat 的输入事件、输出事件、职责边界都已定义。
- 校验：新 preset 通过 `preset_lint`（isolation + schema parity + topic-format + ownership + flow）与全量基线；BDD guard 场景断言完整事件链。

---

## 范围边界（Scope Boundaries，非目标）

- 不做 per-unit 迭代拆分（`tasks.enabled: false`，整份执行）。
- 不单独起 validator hat（全绿测试门槛内建在 executor 的 TDD DoD 里）。
- 不做对齐回环 / 复杂重试 / fix→re-review 循环（对齐失败只记残留）。因此**不需要** `fix_round` dedup 机制。
- 评审**不用并行 subagent**，改成串行维度 hat 链（用户 2026-07-02 修正）；也不引入 coordinator 循环，用扁平直链。
- 不做多消费者 topic / 并行 hat / wave。
- 不改动或替换 `ce-executor-serial`。

---

## 关键决策（Key Decisions）

- 评审结构 → **6 个 dimension-reviewer hat 的串行单链路 + 1 个 synthesizer**（用户 2026-07-02 修正）。维度沿用 `ce-executor-serial` 的 6 维。理由：事件链清晰、纯事件驱动、绕开 subagent 后端依赖；代价是 hat 数变多，用户已接受此取舍。**放弃**了先前「单 hat + 并行 subagent」方案。
- 扁平直链而非 coordinator 循环 → 每维 hat 直接触发下一维（单链路），不要 `ce-executor-serial` 的 coordinator↔dimension 循环。
- 对齐失败 → **记录残留并继续**（用户选定），而非回退补做或阻断。
- 执行 → **TDD 模式整份执行 + 全量测试全绿硬门槛**（用户修正）。不单独起 validator hat；「一把执行」指不做 per-unit 拆分，而非放松测试。
- Preset 命名 = `ce-executor-pipeline`（用户确认）。

---

## 依赖 / 假设（Dependencies / Assumptions）

- 依赖既有 preset 结构范式：`presets/en/merge-loop.yml`（isolated + `tasks.enabled:false` + 单 sequence flow 的最小骨架）与 `presets/en/ce-executor-serial.yml`（hat 字段形态、6 维度评审的 dimension-reviewer/synthesizer 模板）。
- **实现 carrying cost（提醒）**：新增一个 builtin preset 需同步下游清单——`presets/en/<name>.yml`、`presets/manifest.yml`、`presets/index.json`、`crates/ralph-cli/src/presets.rs` 的 `PRESETS` 数组 + 计数/镜像测试、`scripts/ralph-zsh-plugin.zsh`、`AGENTS.md`/`CLAUDE.md`、`.cursor/rules/multi-hat-isolation.mdc`，最后跑全量 `./scripts/run-tests.sh`。
- **WAC egress 风险**：egress 闭包 BFS `EGRESS_MAX_HOPS=4`，而本 preset 是 ~12 跳的扁平长链，极可能触发 `activation_egress_missing`；处置见开发计划（大概率需 `topology_exempt` 白名单放行，链确实能终止，属合法豁免）。
- **质量风险（已缓解）**：executor 的「全量测试全绿」是硬门槛（R7），坏代码在执行阶段就被拦；6 维度评审 + testing/correctness 维度实际跑测试是第二道。
- 评审不再依赖 hat 内 spawn subagent（已放弃该方案），故不再有 subagent 后端可行性假设。

---

## 待解问题（Outstanding Questions）

### 留到计划阶段（Deferred to Planning）
- [影响 R2/R8][技术] 6 个维度 hat 之间的具体事件名与 topic-format 合规（是否需进 `topic_format_whitelist`）、以及维度产物文件的落盘路径（遵守 `ephemeral_isolation`）。
- [影响 R2][需研究] WAC egress 闭包对 ~12 跳长链的实际判定与处置（`topology_exempt` 白名单 vs 其它）。
- [影响 R6/R7][技术] executor 如何自动发现测试入口并断言全绿（对齐项目 `cargo nextest` 硬规则；跨语言泛化）。
- [影响 R1/R3][技术] `tasks` 机制关闭（`tasks.enabled:false`）后 `state_projection`/`execution_contracts` 的最小配置。
- [影响 R13][技术] 报告文件与各维度产物文件的落盘位置与格式。

---

## 下一步（Next Steps）

-> 开发计划已随本次修正重写：`docs/plans/2026-07-02-003-feat-ce-executor-pipeline-preset-plan.md`（`-> /ce-work` 实现）；实现落地建议先用 U1 骨架跑 `preset_lint` 把 WAC egress 处置定下来。
