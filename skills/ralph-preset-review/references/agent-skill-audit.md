# Agent-Skill Audit（可选，弹窗选择）

> **触发**：review SKILL Workflow 第 0a 步弹窗选项 2「同时审查注入 skill 文档」被选中；默认（推荐）**跳过**。本规程默认不跑，仅选审时跑。
> **目标**：把 `crates/ralph-core/data/*.md` 这类注入给 agent 的 skill 文档当作「审查对象」，按下面 finding_id 找出口径不匹配 / 可读性差 / 假称已注入等违规。

## 1. 审计源（外仓注意）

- **本仓**：`crates/ralph-core/data/*.md`（来源在 git tree 内，diff 一目了然）。
- **外仓**（无 `crates/ralph-core/data/`）：审计内容 = 当前 ralph 二进制内嵌的 skill 文档（`SkillRegistry` 的 `include_str!`）。审完后**报告必须写明来源**是「二进制内嵌」，并注明 `ralph tools skill load <name>` / `ralph inspect prompt` 的输出来源；不要让 reviewer 把内嵌内容误以为本仓 `data/*.md`。

## 2. 审计步骤（选审才跑）

1. 列出所有 builtin skill：`ralph tools skill list --format json`（或在 SKILL/agent 上跑 `ralph inspect prompt --hat ralph --format json` 拿全名单）。
2. 对每个 skill（按名字），按下面「finding_id 表」逐条判读。
3. 命中按 `references/finding-rubric.md` 的 `agent_skill.*` ID + default severity + default confidence 入主表；confidence < 60 → `Unverified Suspicions`。
4. 在报告 Executive Summary 写 `agent_skill_audit: performed`，并在「来源」段注明「本仓 / 二进制内嵌」。
5. 若发现某 skill 的内容**与 hat `instructions:` 互相矛盾**（如 `instructions:` 说「`ralph-tools-emit` 已自动注入」而 `data/ralph-tools-emit.md` 顶部明确是 on-demand），直接升 P0（`agent_skill.inject_claim_false`）。

## 3. finding_id 表

| finding_id | default_severity | default_confidence | aaf_question | category | 含义 |
|---|---|---|---|---|---|
| `agent_skill.leaks_internals` | P0 | 95 | Q3 | lint | skill 文档泄漏了运行时不应让 agent 看到的内部实现细节（内部函数名 / 模块名 / 内部 ledger 路径 / 不可见的 review-only 注释 / 一次性事故报告路径 / 过窄 preset 案例）。凡「agent 不可见但被印出来」的内部路径或代码定位都属于此。 |
| `agent_skill.unreadable` | P1 | 85 | Q3 | style | skill 文档可读性差：术语首次出现未解释（`hat` / `topic` / `task_key` / `step` / `task_id` / `kind` / `reason` / `allowed_topics` / `required_fields` / `policy-check` 等）、未按「agent 下一步能执行什么」写、未说明触发条件与失败停止条件等。 |
| `agent_skill.inject_claim_false` | P0 | 95 | Q3 / Q4 | lint | skill 文档 / hat `instructions:` 错误声称某 skill 已自动注入，或把 on-demand skill 写成 auto-inject。对账源：`ralph inspect prompt --hat <id> --format json`。 |


## Scope contract skill audit（选审时检查）

当 reviewer 选择「同时审查注入 skill 文档」时，按以下四条检查 `crates/ralph-core/data/ralph-tools-emit.md` 的 scope handoff contract 段：

| 检查项 | finding_id | default_severity | default_confidence | 含义 |
|---|---|---|---|---|
| `ralph-tools-emit.md` 缺少 scope topics 列表（`merge.integrated` / `merge.stabilized` / `postmerge.changemap.ready` / `redteam.plan.resolved`） | `agent_skill.unreadable` | P1 | 85 | skill 文档缺少关键触发条件 |
| `ralph-tools-emit.md` 未说明 `--unsafe-no-policy-check` 不能绕过 scope handoff guard | `agent_skill.leaks_internals` | P0 | 95 | skill 文档未说明关键约束，导致 agent 可能误用 |
| `ralph-tools-emit.md` 未说明 `scope_digest` 是排除自身字段的 SHA-256 | `agent_skill.unreadable` | P1 | 85 | 关键算法描述缺失 |
| `ralph-tools-emit.md` 未说明 threshold gate（`overall_confidence >= 90` + `critical_unknown_count == 0` + `proceed == true`） | `agent_skill.unreadable` | P1 | 85 | 关键成功条件描述缺失 |
## 4. 与其它 finding_id 的边界

- **不去替代** `preset.instructions_opac_skill_reference_missing` 等既有 finding——后者是 lint 抓的 shape 缺失；本表是 review-only 的「内容口径」层。
- **不**检查「运行时门控改变」——那是 `preview_characterization` 测试（ralph-core::event_loop::tests）与 `ralph inspect prompt` JSON SSOT 的责任。
- **不**去审用户安装的 marketplace skill——本规程只覆盖 builtin / 内嵌。

## 5. 反模式（出现即重写）

- **泄漏** `.ralph/events.jsonl` / `.ralph/loops.json` / `.ralph/supervisor.db` 给 agent 当作可读输入路径。
- **泄漏** `check_*` / `*_guard` / `recovery_runtime::*` 等内部函数 / 模块名给 agent 用作「下一步动作」依据。
- **泄漏** review-only 注释 / 一次性事故报告（`.ralph/review/<plan>/diagnosis.md` 等）给 agent 当事实源。
- **去计划化**：在 skill 文档里写只适用于某次事故 / 某 plan / 某过窄 preset 的内容。通用 skill 必须是可复用约束，不绑死具体场景。
- **假称已注入**：在 hat `instructions:` 里说「skill X 已自动注入」或「请直接使用 skill X」，但 skill X 实际是 on-demand 且 `ralph inspect prompt` 把它列在 `on_demand[]` 而非 `auto_inject[]`。

## 6. 报告字段

- `agent_skill_audit: performed`（Executive Summary）
- 「来源: 本仓 `crates/.../data/*.md` / 二进制内嵌」（Executive Summary）
- 命中表（同主表结构：`id | severity | confidence | aaf_question | category | hat | location | evidence | problem | fix`）
- 每个 `agent_skill.*` finding 的 `evidence` 段必须引用 `ralph inspect prompt --format json` 输出片段或具体 skill 文档路径 + 行号范围。

## 7. 与 prompt-visibility 规程的关系

- `prompt-visibility.md`（共享）覆盖 **「该 hat 这一轮真看到什么」**——`auto_inject` vs `on_demand`。
- 本规程覆盖 **「注入的 skill 文档内容是否合规」**——`agent_skill.*` finding。
- 两者相辅：先用 `inspect prompt` 拿可见性证据，再据本规程逐 skill 审内容。