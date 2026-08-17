# Prompt Visibility（共享规程）

> **共享**：author / review / diagnose 三套 skill 都引用本文件。
> **仅改 `skills/`**：本文件路径在 `skills/ralph-preset-author/references/`（review skill 持有同一份的字节一致副本，覆盖 install 树比较契约）；**编辑时必须同步改动 author 与 review 两份**。**禁止**把本文件写入 `.claude/skills/**`（那是 getaLawyer / 安装树副本）。

## 1. 为什么需要这条规程

`event_loop.execution_mode: isolated` 下，每条 hat activation 只看自己的 `instructions:` + runtime 自动注入的 skill。**Agent 是否真看到**某条 skill 由三件事决定：

- `tasks.enabled` / `memories.enabled`（门控）
- `SkillRegistry` 里该 skill 的 `auto_inject` flag（registry 路径）
- hat `frontmatter.hats`（per-hat 限制，U8 修复后才会过滤）

仅靠记忆「这个 skill 应该在 auto-inject 里」极易错。**`ralph inspect prompt` 是同源只读命令**，与 `EventLoop::build_prompt` 走同一份逻辑（`ralph-core::event_loop::preview_prompt_for_config`），所以：

> **起草 / 改 instructions 之前**先跑 `ralph inspect prompt`，拿结构化输出（`auto_inject` / `on_demand` / `block_titles`）作为唯一可见性证据。

禁止凭经验把 on-demand skill 写成「已自动注入」。`ralph inspect prompt --format json` 是 SSOT，任何工具 / 合同测都优先用它。

## 2. 命令模板

```bash
# 默认 human 输出（块清单 + skill 表）
ralph -c <preset>.yml inspect prompt --hat <hat_id>

# JSON（合同 / fixture / lint 自动化必用）
ralph -c <preset>.yml inspect prompt --hat <hat_id> --format json

# 外仓（无 crates/）同样适用
ralph -c ./local.yml inspect prompt --hat worker --format json
```

字段约定（见 `references/commands.md` 同步登记）：

| 字段 | 含义 |
|---|---|
| `hat_id` | 当前 hat id（来源：`--hat` 或 `RALPH_CURRENT_HAT`） |
| `gates.tasks_enabled` | 注入 `ralph-tools` / `ralph-tools-tasks` 的门控之一 |
| `gates.memories_enabled` | 注入 `ralph-tools` / `ralph-tools-memories` 的门控之一 |
| `auto_inject[]` | **已自动注入**的 skill + source (`gated` \| `registry_auto`) |
| `on_demand[]` | 可见但**未自动注入**的 skill，按名字排序 |
| `block_titles[]` | prompt 里 `## …` 块标题，按出现顺序 |
| `prompt_body` | `--full` 时返回真实 instructions + 注入 skill 拼接的完整 body；human 模式打印同等文本（不 suppressed） |

## 3. auto vs on_demand 判定

`auto_inject` 与 `on_demand` 互补（并集 = 该 hat 可见的全部 skill）：

- **`gated`**：`ralph-tools` / `ralph-tools-tasks` / `ralph-tools-memories` / `ralph-tools-opac`，由硬编码门控 + 注册表可见性决定
- **`registry_auto`**：registry frontmatter 标 `auto_inject: true`，且被当前 hat 可见（per-hat `hats` 限制允许）
- **`on_demand`**：其它可见 skill，必须用 `ralph tools skill load <name>` 加载

写 instructions 时：

- 引用自动注入的 skill → **直接引用** `ralph-tools.md` / `ralph-tools-emit` 等名字（不要让 agent `ralph tools skill load` 它们）
- 引用 on-demand skill → **明确**写「先 `ralph tools skill load <name>` 再按 §X 做」

## 4. 外仓（outer repo）注意

- 没有 `crates/ralph-core/data/` 时，`ralph inspect prompt` 仍可用——内容来自**当前 ralph 二进制内嵌**（`crates/ralph-core/src/skill_registry.rs` 的 `include_str!`）
- 若报告里写「本仓 vs 外仓注入差异」，必须**注明来源是二进制内嵌**，不要让 reviewer 以为来源是 repo 内 `data/*.md`
- 外仓对 `data/*.md` 的本地修改**不会**反映到 inspect 输出（除非重装 skill 树覆盖）

## 5. 与三 skill 的接缝

| Skill | 引用本文件的位置 | 必走动作 |
|---|---|---|
| `ralph-preset-author` | Workflow 起草 hat `instructions:` 之前 | 跑 `ralph inspect prompt` 确认该 hat 真看到的 skill |
| `ralph-preset-review` | Per-hat AAF Q2/Q3（Visible context） | 对每条 hat 跑 `inspect prompt` 作为可见性证据 |
| `ralph-run-diagnosis` | 怀疑「agent 看不到某 skill」对账 | 跑 `inspect prompt` 对账 `ralph tools skill list` |

更细的 audit 规程见 `references/agent-skill-audit.md`（U10，弹窗默认不审 data/*.md，仅选审时跑）。


## Scope resolution is agent-owned

Scope 解析（`merge.integrated` / `merge.stabilized` / `postmerge.changemap.ready` / `redteam.plan.resolved`）是 **agent-owned** 能力：三套独立 scope preset 的 hat 必须自己从 Git 历史和 artifact 内容独立推算 scope，不得依赖 operator 提供的 `ralph scope` CLI 或外部工具。

**preset 作者规则**：
- **禁止**在 hat `instructions` 中引入 `ralph scope` 命令或任何 scope-specific CLI。
- scope topic 的 hat `instructions` 必须引用 `ralph-tools-emit.md` 的 scope handoff contract 段，描述如何写 manifest、算 digest、跑 policy-check。
- `scope_base_sha` / `scope_digest` / `scope_manifest_path` 等字段在 `instructions` 中必须说明来源（从 Git / artifact / 工作目录取得），不得写「上游会处理」。
- `--unsafe-no-policy-check` 不能绕过 scope handoff guard，这一约束必须在涉及 scope topic 的 hat `instructions` 中明确说明。
<!-- anchor: evidence-bound -->

Recovery guidance 出现在 target hat 的 `## ORCHESTRATOR CORRECTION` 中，位于 Observed / Invariant / Must re-prove 之后。agent 只能看到 runtime 注入的 Common / Check-specific 段，看不到 `.ralph/events.jsonl` 或 preset lint 内部函数。guidance 不是成功 payload。readonly hat 的 workspace 证据应写在 `.ralph/**`，不要让 hat 去读 runtime ledger。