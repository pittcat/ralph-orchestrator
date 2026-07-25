---
name: ralph-run-diagnosis
description: >-
  Post-run deep diagnosis for any Ralph preset loop. Inventories .ralph artifacts
  by tier (S/A/B/C), reconciles events/ledger/recovery/logs against preset schema,
  audits OPAC with mode-aware confidence, traces mechanism bugs to source lines,
  optionally correlates against prior docs/report/ docs/plans/ docs/solutions/
  docs/brainstorms/ (opt-in via --include-history; off by default), writes
  docs/report/*-diagnosis.md with per-finding root-cause confidence scores and
  low-confidence re-investigation. Use after ralph run, ralph-e2e, debug.md,
  loop diagnosis, or orchestration vs mechanism.
argument-hint: "[run_dir] [preset_file_or_builtin] [optional: plan_file] [--include-history disabled|preset-only|full]"
---

# Ralph Run Diagnosis

跑后诊断：**先盘点产物 → 按 Tier 对账 → 历史 → 源码归因 → 落盘报告**。不修代码。

**写任何机制/路径前必读**：[ssot-guardrails.md](references/ssot-guardrails.md)（禁止 hat_handoff、loop_state_snapshot.json、错误 CLI 等）。

**交付物**：**主仓** `docs/report/YYYY-MM-DD-<preset>-<loop_id>-diagnosis.md`。

> **变更日志**：
> - 2026-07-25：新增 §0.1 历史检索开关 + SSOT 常量 + §0.x 编号扩展约定。若主项目文档（CLAUDE.md / AGENTS.md / operator docs）需说明此开关，在此 `变更日志` 行追加即可（不触发 CLAUDE.md/AGENTS.md 强制同步——本 skill 不属于 `crates/ralph-core/data/`）。

## 0.1 历史检索开关（HARD RULE）

**默认 `--include-history=disabled`**。Phase 0 完成《产物盘点表》之后，**必须**用 `AskUserQuestion` 询问一次（三选一，默认 disabled）：

> 历史扫描会跨入主仓 `docs/report/` / `docs/solutions/` / `docs/plans/` / `docs/brainstorms/`，**不属于**本次 run 的 `.ralph/` 范围。
>
> 1. **不检索（disabled，默认）** — 只看本次 run 产物；§3 / §5 历史关联列一律写 `N/A (history disabled)`；**跳过 Agent B 与 L5**。
> 2. **本次 preset/loop 历史（preset-only）** — 仅扫描与 `preset` / `loop_id` 关键词相近的 30 天滑动窗口条目。
> 3. **全库（full）** — 同 2 但窗口扩到全库；用于复发排查 / compound 归因。

**参数与询问互斥说明**：human 在调起 skill 时**可显式传** `--include-history`（见 §1 参数表）；若 human 未传且已授权交互（默认假设），agent 在 Phase 0 后**应用 `AskUserQuestion` 询问一次**。两种入口殊途同归，最终 `--include-history` 值以最近一次确认为准（参数 > 询问）。

**禁用默认开启的理由（hard rule）**：

- 历史检索会跨入主仓，**不属于**本次 run 的 `.ralph/`；未确认授权前不得假设这是用户期望。
- 复用旧报告的根因分类可能产生跨 preset 误归因（已多次发生；参见 `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` 的"静默 no-op"教训——同构问题）。
- 扫描窗口不设上限会随 preset/symptom 关键词数无限扩张，且需读非本次 run 的 `.ralph/` 之外目录；用户在确认是否启用前不应承担这部分耗时。

**启用后的纪律（仅 `--include-history ≠ disabled` 时生效）**：

- Agent B 才允许启动；L5 才允许跑；`confidence-rubric` 加深顺序第 4 项"历史对照"才有效。**disabled 模式下第 4 项标记为不可用**（不扫描、不写结论），详见 [confidence-rubric.md](references/confidence-rubric.md)。
- Agent A / C / D 在 disabled 模式下**禁止读** `docs/report/`、`docs/solutions/`、`docs/plans/`、`docs/brainstorms/`（即便不出现在产物中也禁止扫描）。详见 [subagent-charters.md](references/subagent-charters.md)。
- 报告 §3 / §5"历史关联"列才允许填值；否则一律写 §0.1-占位符。
- 报告 frontmatter 必须记录 `history_search: disabled | preset-only | full`，与 `execution_capabilities` 同级。

提交前 checklist 多一条：**历史检索开关状态已写入 frontmatter**。

### SSOT 常量

为避免字面散落多处漂移，约定以下占位符/标签为单一事实源：

| 名称 | 字面值 | 用途 | 出现位置（不得复制变体） |
|------|--------|------|----------------------------|
| **§0.1-占位符** | `` `N/A (history disabled)` `` | §3 / §5 disabled 模式下历史关联字段 | report-template.md §3 / §5 |
| **样式标签** | `> **⚠️ 启动条件**：…` | 4 份 references 内的"启动条件"提示块 | history-sources.md / report-template.md §3 / subagent-charters.md Agent B / verification-pipeline.md L5 |
| **§0.1 链接锚** | `[SKILL.md §0.1 历史检索开关](SKILL.md#01-历史检索开关hard-rule)` | 跨 references 互引主文档 | 任意引用主 SKILL 的 §0.1 处 |

未来若需调整字面，**必须先**在本表登记，再改动引用处（references 间交叉互引但以本表为字面 SSOT）。

### §0.x 编号扩展约定（HARD RULE）

`## 0.x` 仅用于"必须先于 `## 1. 输入`"的全局开关段（Phase 0 类配置 gate）。若未来新增 `§0.y`：

- 必须是另一组**全局门控开关**（如 `--strict-mode`、`--dry-run` 等），不是局部指令。
- 顺序按门控依赖排列：`§0.1` (history) → `§0.2` (xx) → ... → `## 1. 输入`。
- 不得与 `## 1.` 之后的章节混排；一律放在「输入」之前。

---

## 1. 输入

| 参数 | 必填 | 说明 |
|------|------|------|
| `run_dir` | 是 | 含 `.ralph/` 的 workspace（可 sibling worktree） |
| `preset` | 是 | `presets/en/foo.yml` 或 `builtin:foo` → 解析为 `presets/en/foo.yml` |
| `plan_file` | 否 | plan frontmatter 对账 |
| `repo` | 否 | 默认当前 `ralph-orchestrator` 主仓（报告路径） |
| `--include-history` | 否（默认 disabled） | `disabled` / `preset-only` / `full`；见 §0.1 |

## 强制四问（§1 逐条，禁止合并）

1. 执行与 OPAC（须标 **diagnostics 模式 + OPAC 置信度**）
2. 基座机制是否生效
3. 编排是否合理
4. 归因：preset / mechanism / agent / compound（附 **根因置信度**）

## 强制对账：prompt visibility（hat 这一轮真看到什么）

> **触发条件**：诊断怀疑「agent 看不到某 skill」或「agent 引用了不该看到的内部实现」时，**必须**在 Phase 0 之后、Phase 1 之前用 `ralph -c <preset> inspect prompt --hat <id> --format json` 跑一次可见性对账。对账源即 [../ralph-preset-common/references/prompt-visibility.md](../ralph-preset-common/references/prompt-visibility.md) 的 `auto_inject` / `on_demand` / `block_titles` 字段。
>
> **对账要点**：
>
> 1. **auto vs on-demand 矛盾**：hat `instructions:` 把 on-demand skill 当成 auto-inject 用 → `agent_skill.inject_claim_false`（见 [../ralph-preset-common/references/finding-rubric.md](../ralph-preset-common/references/finding-rubric.md)）。
> 2. **skill 文档泄漏内部实现**：auto_inject 的 skill 内容含内部函数名 / 模块名 / 内部 ledger 路径 / review-only 注释 → `agent_skill.leaks_internals`。
> 3. **Confirm 路径与 `ralph tools skill load` 期望**：`on_demand[]` 里有 skill 但 hat `instructions:` 没要求 agent 先 `ralph tools skill load` → 行为缺口，按 Q3 入栏。
>
> 报告 §1「强制四问」答完后，附一段「**Prompt visibility 对账**」，引用 `inspect prompt` JSON 关键字段（`auto_inject[].name` / `on_demand[].name`），不要复述 prompt 全文。

## 执行顺序（硬约束）

```
Phase 0 盘点（串行，主 Agent）
    → 产出《产物盘点表》+ diagnostics 四档
    → AskUserQuestion 决定 --include-history（详见 §0.1）
    → 仅 then ↓
Phase 1  A∥[B 仅在 --include-history ≠ disabled]（流程 + 历史）
Phase 2  C（对账，吃 A+B+盘点表）
Phase 3  D（归因 + 置信度评分，吃 C+B+源码；低分加深）
Phase 4  主 Agent 汇总落盘
```

**禁止**在 Phase 0 完成前启动 sub-agent。

## Phase 0

[artifact-discovery.md](references/artifact-discovery.md) 六步 + [artifact-manifest.md](references/artifact-manifest.md) 分层读：

- **Tier S**：`current-events` → **唯一** events 文件（禁止 `events*.jsonl` 通配）
- **Tier A**：tasks/progress/summary/handoff（后两者仅终止后）
- **Tier B/C**：按盘点表 + preset/schema 解析

Diagnostics 四档：`FULL` | `MINIMAL` | `LOGS_ONLY` | `DISABLED` — 决定 L2/L OPAC 深度。

### Phase 0 能力推断（execution capabilities）

> **目的**：在写报告 §0 与 §1 之前，先声明这次 run 的 capability 集合，便于后续对账（supervisor.db 是否存在、wave_id Confirm 走哪条路径）有锚点。**禁止**按 builtin preset 名称点名门控；一律 capability-triggered（Intent.execution_model + YAML 信号 + 产物信号）。

**推断步骤（顺序固定）**：

1. 读 [`../ralph-preset-common/references/agent-native-model.md`](../ralph-preset-common/references/agent-native-model.md)「执行模型（Execution Model）」段确认枚举与检测信号；该节是 frozen vocabulary，本 plan 不再扩展。
2. 解析 preset：
   - `event_loop.supervisor.enabled: true` → capability +supervisor
   - hat `instructions` 含 `ralph wave emit` / `ralph wave verify`，或 hat 依赖 `## WAVE CONTEXT` → capability +wave
   - **禁止**用 `exec.wave.*` / `slot.*` 等协调 topic 推断 +wave（那些是 supervisor 协调面，走 supervisor audit，不是 wave fan-out 信号）
3. 解析 Intent（如有作者 notes）：`execution_model: wave | supervisor | supervisor+wave` → 与上面 capability 一致则 OK；不一致 → 主表 P0（详见 [`../ralph-preset-common/references/finding-rubric.md`](../ralph-preset-common/references/finding-rubric.md)「Supervisor capability audit」段 `preset.execution_model_intent_mismatch`）。
4. 扫描产物与 Observe 门控：
   - `.ralph/supervisor.db` 存在 → ledger 证据。若 YAML 已 `supervisor.enabled: true`，加固 +supervisor；若 enabled=false（常见 default-wave），**不要**因此否定 `ralph inspect loop` 可能出现的 `supervisor` 键——键在 **enabled 或盘上已有可打开 wave 账本** 时都会出现；先 `jq 'has("supervisor")'` 再读块
   - events 含 `wave_id` → capability +wave
5. 输出到报告 §0 的 **`execution_capabilities`** 字段（字符串数组），例如 `["single-chain"]` / `["wave"]` / `["supervisor", "wave"]`。

**缺 db / 缺 wave_id 不算故障（hard rule）**：在 capability 推断结果为单链时，缺 `.ralph/supervisor.db` 是**预期**，**不**是异常；events 无 `wave_id` 也是**预期**，**不**是异常。**仅**当 capability +supervisor（YAML `supervisor.enabled: true`）时缺 db 才列为缺失（runtime 异常）；**仅**当 capability +wave 时缺 wave_id 对账才列为缺失。`inspect` JSON **无** `supervisor` 键：仅当 enabled=false **且** 盘上无 ledger 时为预期；不要把「enabled=false」单独当成「必无 supervisor 键」。

**wave Confirm 路径**：capability +wave（产物侧常见 `wave_id`）时，worker / dispatcher 完成态由 `ralph events --events-source main`（main ledger）对账；hat-channel 是 dispatcher 自己 private 落盘点，**不**用作 wave Confirm。L3 / L4 验证按 `references/mechanism-checklist.md`（如有 wave Confirm 源行则引用）。

## Phase 1–3 Sub-Agent

见 [subagent-charters.md](references/subagent-charters.md)、[verification-pipeline.md](references/verification-pipeline.md)。

**根因置信度**（详见 [confidence-rubric.md](references/confidence-rubric.md)）：

- **§5 入表门槛**：confidence ≥ 60；**P0 须 ≥ 70**，否则继续深挖或降为 P1
- **低分强制加深**：< 60 不得写入 §5 定论；按 rubric 补读 recovery/源码/preset 行号/历史，最多 2 轮
- **仍不足**：移入 §7「未核实疑点」，不得写修复建议
- 有 `file:line` + 双账本一致 → 可 ≥85；LOGS_ONLY 下 OPAC/agent 单项 ≤50
- compound 须写贡献比例 + 各成分置信度

**OPAC 置信度**：[opac-audit-by-mode.md](references/opac-audit-by-mode.md)

## Phase 4 落盘

[report-template.md](references/report-template.md)；§0 产物盘点 + §1 四问 + 盲区声明。

## 提交前检查

- [ ] Phase 0 盘点表在报告中
- [ ] 只读了 `current-events` 指向的 events
- [ ] LOGS_ONLY 未因缺 orchestration 标 P0
- [ ] 每条 P0/P1 在 §5 有 **置信度**；P0≥70、入表≥60
- [ ] confidence<60 的候选已加深或落入 §7，未混入 §5/§6
- [ ] 未引用 ssot-guardrails 禁止项
- [ ] 报告在主仓 `docs/report/`
- [ ] **历史检索开关状态已写入 frontmatter**（`history_search: disabled | preset-only | full`）

## 参考

- [ssot-guardrails.md](references/ssot-guardrails.md) — **过时概念/错误路径禁止清单**
- [artifact-manifest.md](references/artifact-manifest.md) — Tier S/A/B/C
- [artifact-discovery.md](references/artifact-discovery.md) — Phase 0
- [confidence-rubric.md](references/confidence-rubric.md) — **根因置信度评分 + 低分加深**
- [log-reconciliation.md](references/log-reconciliation.md)
- [mechanism-checklist.md](references/mechanism-checklist.md)
- [source-trace-guide.md](references/source-trace-guide.md)
- [history-sources.md](references/history-sources.md)
- [examples/minimal-diagnostics-layout.md](references/examples/minimal-diagnostics-layout.md)
- 样板：`docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md`
